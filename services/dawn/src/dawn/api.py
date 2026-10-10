from __future__ import annotations

import asyncio
import contextlib
import logging
import os
import pathlib
import time
from collections.abc import AsyncIterator, Callable
from dataclasses import dataclass, field
from typing import Any

import anyio
import anyio.to_thread
from dusk.gw import ConnectionRegistry
from dusk.gw.mcp import build_application as build_mcp_application
from dusk.gw.rest import MALFORMED, ErrorResponse
from dusk.gw.rest import build_application as build_rest_application
from fastapi import FastAPI, HTTPException, Request
from fastapi.openapi.docs import get_swagger_ui_html
from starlette.applications import Starlette
from starlette.requests import Request as StarletteRequest
from starlette.responses import JSONResponse, Response
from starlette.routing import Mount, Route
from starlette.staticfiles import StaticFiles
from starlette.types import ASGIApp

from . import facts
from .auth import AuthenticationMiddleware, Authenticator
from .config import Settings
from .dispatch import Dispatcher, Draining, QueueFull
from .files import Files, UploadsExhausted
from .gateway import (
    AT_CAPACITY,
    DAWN_INSTRUCTIONS,
    NIGHTFALL_REFUSED,
    TOO_SLOW,
    connect_route,
    connect_tool,
    owner_of,
    principal_of,
    refusal,
)
from .logstreams import LogStreams, StreamsExhausted
from .metrics import Metrics, RequestMetrics
from .models import (
    Accepted,
    DispatchRequest,
    FactsRequest,
    FactsResponse,
    FileAccepted,
    FileRequest,
    LogStream,
    LogStreamRequest,
    LogStreamStarted,
    NodeRef,
    ReapRequest,
    Reported,
)
from .nodes import Connector, Sessions, SessionsExhausted, described
from .telemetry import RequestSpans
from .work import Results, classify, shell_at

logger = logging.getLogger(__name__)

STATIC = pathlib.Path(__file__).parent / "static"
BLANK_FAVICON = "data:,"
SWAGGER_UI_PARAMETERS = {"validatorUrl": None}
TITLE = "dawn"
DESCRIPTION = """
Run processes on dusk nodes through nightfall.

Every call that reaches a node names it with a node reference and a pid, the
process the work runs as. `POST /dispatch` runs work and reports its results to
Kafka; `POST /reap` kills and removes processes; `/facts`, `/logs` and `/files`
read a node's facts, stream its logs to a collector and collect its files.
`POST /connect` opens an interactive session; pass its descriptor to `/sh`.

AI agents can drive the same sessions through the MCP server at `/mcp`.
"""
BUSY_NODE = {
    "model": ErrorResponse,
    "description": "The node already has as many processes or reaps waiting as dawn "
    "queues for one node, or dawn holds as many waiting scripts as it may.",
}
TOO_MANY = {
    "model": ErrorResponse,
    "description": "dawn already runs as many of these as it may.",
}
GONE = {
    "model": ErrorResponse,
    "description": "The node could not be reached through nightfall.",
}
NO_SUCH_STREAM = {"model": ErrorResponse, "description": "No log stream has that id."}


@dataclass
class Services:
    settings: Settings
    authenticator: Authenticator
    connector: Connector
    sessions: Sessions
    results: Results
    dispatcher: Dispatcher
    files: Files
    log_streams: LogStreams
    metrics: Metrics
    programs: list[dict[str, Any]]
    kafka_ready: Callable[[], bool] = lambda: True
    clock: Callable[[], float] = time.time
    draining: bool = False
    registry: ConnectionRegistry = field(default_factory=ConnectionRegistry)
    interactive: set[tuple[str, str, int]] = field(default_factory=set)

    def check_address(self, node: NodeRef) -> None:
        if node.nightfall is not None and not self.settings.inner_address_allowed(
            node.nightfall
        ):
            raise HTTPException(
                400,
                f"nightfall {node.nightfall} is not in nightfall.allowed_inner_addresses",
            )

    def check_accepting(self) -> None:
        if self.draining:
            raise HTTPException(503, "dawn is shutting down and takes no new work")

    def readiness(self) -> dict[str, bool]:
        return {"kafka": self.kafka_ready(), "accepting": not self.draining}


def add_routes(api: FastAPI, services: Services) -> None:
    settings = services.settings

    @api.post(
        "/dispatch",
        status_code=202,
        summary="Run work on a node",
        description=(
            "Queues the work on the node the reference names and answers at once. Each "
            "piece of work runs once, as the process at its pid: a shell server there "
            "runs its script. When that pid is already in the node's process table, "
            "nothing runs and the result is `duplicate`. Results arrive on the "
            "`dusk.process-results` Kafka topic, output on `dusk.process-output`. A "
            "node runs one dispatch at a time; work sent while one runs waits and runs "
            "together after it, quarantine first, then scripts, then configuration, "
            "then versions. A pid that is already waiting or running is accepted "
            "without running twice."
        ),
        response_description="The pids dawn accepted.",
        responses={400: MALFORMED, 429: BUSY_NODE, 503: AT_CAPACITY},
    )
    async def dispatch(body: DispatchRequest) -> Accepted:
        services.check_address(body.node)
        services.check_accepting()
        try:
            accepted = services.dispatcher.submit(body)
        except ValueError as failure:
            raise HTTPException(400, str(failure)) from failure
        except QueueFull as failure:
            raise HTTPException(429, str(failure)) from failure
        except (SessionsExhausted, Draining) as failure:
            raise HTTPException(503, str(failure)) from failure
        return Accepted(accepted=accepted)

    @api.post(
        "/reap",
        status_code=202,
        summary="Kill and reap processes on a node",
        description=(
            "Queues the pids on the node the reference names and answers at once. "
            "Each process is stopped and removed from the node's process table, in "
            "turn with the node's dispatches; each pid gets one `reaped` result on "
            "`dusk.process-results`."
        ),
        response_description="The pids dawn accepted.",
        responses={400: MALFORMED, 429: BUSY_NODE, 503: AT_CAPACITY},
    )
    async def reap(body: ReapRequest) -> Accepted:
        services.check_address(body.node)
        services.check_accepting()
        try:
            accepted = services.dispatcher.submit_reap(body)
        except QueueFull as failure:
            raise HTTPException(429, str(failure)) from failure
        except (SessionsExhausted, Draining) as failure:
            raise HTTPException(503, str(failure)) from failure
        return Accepted(accepted=accepted)

    @api.post(
        "/facts",
        summary="Read a node's facts",
        description=(
            "Reads every `dusk.*` fact the node registered, the version keys you name, "
            "its configuration hash and the processes it runs, in a shell at `pid`, "
            "and answers with them. `dusk.device.id` is never read out. The same "
            "result goes to `dusk.process-results` with the action `collect_facts`."
        ),
        response_description="The node's facts and the state it reports.",
        responses={
            400: MALFORMED,
            403: NIGHTFALL_REFUSED,
            502: GONE,
            503: AT_CAPACITY,
            504: TOO_SLOW,
        },
    )
    async def collect_facts(body: FactsRequest) -> FactsResponse:
        services.check_address(body.node)
        services.check_accepting()
        try:
            services.sessions.reserve()
        except SessionsExhausted as failure:
            raise HTTPException(503, str(failure)) from failure
        node = body.node
        attempt = services.results.attempt(
            node, int(body.pid), None, None, "collect_facts"
        )
        failure: Exception | None = None
        found: dict[str, Any] = {}
        reported: dict[str, Any] = {}
        try:
            async with asyncio.timeout(settings.limits.process_timeout_default):
                async with shell_at(
                    services.connector, settings, node, attempt.pid, attempt.log()
                ) as connection:
                    attempt.delivered = True
                    found, reported = await facts.collect(connection, body.version_keys)
            attempt.status = "succeeded"
            attempt.reported = reported
        except asyncio.CancelledError:
            attempt.error = "dawn stopped before the facts were read"
            await services.results.finished_despite_cancellation(attempt)
            raise
        except Exception as caught:
            failure = caught
            attempt.status = classify(caught, attempt.delivered)
            attempt.error = described(caught)
        finally:
            services.sessions.release()
        await services.results.finished(attempt)
        if failure is not None:
            raise refusal(failure) from failure
        return FactsResponse(facts=found, reported=Reported(**reported))

    @api.post(
        "/logs",
        status_code=202,
        summary="Stream a node's logs to a collector",
        description=(
            "Streams the node's logs at the level you name and above to an "
            "OpenTelemetry collector for `duration_seconds`, from a shell at `pid`: the "
            "configured collector, or `endpoint` when it is one dawn allows. Answers at "
            "once with the stream's id; `DELETE /logs/{stream_id}` stops it sooner."
        ),
        response_description="The id of the stream.",
        responses={400: MALFORMED, 429: TOO_MANY, 503: AT_CAPACITY},
    )
    async def start_logs(body: LogStreamRequest, request: Request) -> LogStreamStarted:
        principal = principal_of(request)
        services.check_address(body.node)
        if body.endpoint is not None and not settings.collector_endpoint_allowed(
            body.endpoint
        ):
            raise HTTPException(
                400, f"endpoint {body.endpoint} is not in collector.allowed_endpoints"
            )
        if body.duration_seconds > settings.limits.max_log_stream_seconds:
            raise HTTPException(
                400,
                f"a log stream lasts at most {settings.limits.max_log_stream_seconds} s "
                "(limits.max_log_stream_seconds)",
            )
        services.check_accepting()
        try:
            stream_id = services.log_streams.start(body, principal.subject)
        except StreamsExhausted as failure:
            raise HTTPException(429, str(failure)) from failure
        except SessionsExhausted as failure:
            raise HTTPException(503, str(failure)) from failure
        return LogStreamStarted(stream_id=stream_id)

    @api.get(
        "/logs",
        summary="List the running log streams",
        description="Every log stream dawn runs, with the node, level, collector and who started it.",
        response_description="The running log streams.",
    )
    async def list_logs() -> list[LogStream]:
        return services.log_streams.list()

    @api.delete(
        "/logs/{stream_id}",
        status_code=204,
        summary="Stop a log stream",
        description="Stops the stream and waits until the node has stopped sending.",
        responses={404: NO_SUCH_STREAM},
    )
    async def stop_logs(stream_id: str, request: Request) -> Response:
        principal = principal_of(request)
        stream = services.log_streams.get(stream_id)
        if stream is None or (
            principal.acts_for_itself() and stream.principal != principal.subject
        ):
            raise HTTPException(404, f"no log stream {stream_id!r}")
        await services.log_streams.stop(stream_id)
        return Response(status_code=204)

    @api.post(
        "/files",
        status_code=202,
        summary="Collect a file from a node",
        description=(
            "Reads the file at `path` on the node, from a shell at `pid`, and stores it "
            "in the files bucket, answering at once with the collection's id. When it "
            "is stored, a message goes to the `dusk.files` Kafka topic; the outcome "
            "goes to `dusk.process-results` with the action `collect_file`."
        ),
        response_description="The id of the collection.",
        responses={400: MALFORMED, 429: TOO_MANY, 503: AT_CAPACITY},
    )
    async def collect_file(body: FileRequest) -> FileAccepted:
        services.check_address(body.node)
        services.check_accepting()
        try:
            upload_id = services.files.start(
                body, settings, services.connector, services.sessions
            )
        except UploadsExhausted as failure:
            raise HTTPException(429, str(failure)) from failure
        except SessionsExhausted as failure:
            raise HTTPException(503, str(failure)) from failure
        return FileAccepted(upload_id=upload_id)

    api.mount("/static", StaticFiles(directory=STATIC), name="static")

    @api.get("/docs", include_in_schema=False)
    async def docs(request: Request) -> Response:
        mounted_at = request.scope.get("root_path", "").rstrip("/")
        return get_swagger_ui_html(
            openapi_url=f"{mounted_at}/openapi.json",
            title=f"{TITLE} - API reference",
            swagger_js_url=f"{mounted_at}/static/swagger-ui-bundle.js",
            swagger_css_url=f"{mounted_at}/static/swagger-ui.css",
            swagger_favicon_url=BLANK_FAVICON,
            swagger_ui_parameters=SWAGGER_UI_PARAMETERS,
        )


def build_application(services: Services, ip: str) -> ASGIApp:
    os.environ["DUSK_NON_INTERACTIVE"] = "1"
    rest = build_rest_application(
        services.registry,
        services.programs,
        connect=connect_route(services),
        owner=owner_of,
    )
    rest.title = TITLE
    rest.description = DESCRIPTION
    add_routes(rest, services)

    application = build_mcp_application(
        services.registry,
        services.programs,
        ip,
        connect=connect_tool(services),
        instructions=DAWN_INSTRUCTIONS,
    )

    async def healthz(request: StarletteRequest) -> Response:
        return JSONResponse({"status": "alive"})

    async def readyz(request: StarletteRequest) -> Response:
        checks = services.readiness()
        return JSONResponse(
            {"ready": all(checks.values()), "checks": checks},
            status_code=200 if all(checks.values()) else 503,
        )

    application.routes.extend(
        [Route("/healthz", healthz), Route("/readyz", readyz), Mount("/v1", rest)]
    )
    wrapped = application.router.lifespan_context

    @contextlib.asynccontextmanager
    async def lifespan(starlette: Starlette) -> AsyncIterator[None]:
        async with wrapped(starlette):
            try:
                yield
            finally:
                with anyio.CancelScope(shield=True):
                    await anyio.to_thread.run_sync(services.registry.disconnect_all)

    application.router.lifespan_context = lifespan
    return RequestSpans(
        RequestMetrics(
            AuthenticationMiddleware(application, services.authenticator),
            services.metrics,
        )
    )
