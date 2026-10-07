"""The gateway's REST API, served under ``/v1``.

One endpoint per method of the ``Dusk`` Python class, so a REST caller drives a
node exactly as a Python caller does:

===========================  ====================================================
``POST /v1/connect``         ``Dusk(host, port)`` - returns a descriptor
``POST /v1/disconnect``      ``Dusk.disconnect()``
``POST /v1/sh``              ``Dusk.sh(command)``
``POST /v1/sh/stream``       ``Dusk.sh(command)``, one event per value
``GET  /v1/help``            ``Dusk.help()``
``GET  /v1/help/{program}``  ``Dusk.help(program)``
===========================  ====================================================

The API describes itself: ``GET /v1/openapi.json`` is the OpenAPI document
FastAPI generates from the models below, and ``GET /v1/docs`` is the Swagger UI
that renders it.

A descriptor is the handle a node connection is addressed by - eight hexadecimal
digits; ``connect`` mints one and ``disconnect`` and ``sh`` consume it. Every REST-minted descriptor
belongs to the single :data:`~dusk.gw.REST_OWNER`, so any REST caller may use any
REST descriptor.

Every response body is JSON, errors included: :class:`ErrorResponse` with the
status code. ``sh`` blocks until the program finishes; ``sh/stream`` sends each
value as it is produced, which is what a program that never finishes needs.
"""

from __future__ import annotations

import json
import pathlib

# Any is imported at runtime, not only under TYPE_CHECKING: pydantic resolves a
# model's field annotations when it builds the schema, so a name it cannot see
# at runtime leaves the model undefined and the OpenAPI document unbuildable.
from typing import TYPE_CHECKING, Any

import anyio.to_thread
from fastapi import FastAPI, HTTPException, Request
from fastapi.exceptions import RequestValidationError
from fastapi.openapi.docs import get_swagger_ui_html
from fastapi.responses import JSONResponse
from pydantic import BaseModel, ConfigDict
from starlette.responses import Response, StreamingResponse
from starlette.staticfiles import StaticFiles

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Callable

    from . import Connection, ConnectionRegistry

    Owner = Callable[[Request], object]

STATIC = pathlib.Path(__file__).parent / "static"
"""The vendored Swagger UI, served at ``/v1/static``. See its README."""

BLANK_FAVICON = "data:,"
"""What the docs page points its favicon at.

An empty data URI, because the browser must not go and fetch one. FastAPI's
default favicon is an absolute URL to fastapi.tiangolo.com, which would make an
otherwise self-contained page reach the internet for an icon.
"""

SWAGGER_UI_PARAMETERS = {"validatorUrl": None}
"""Turns off Swagger UI's validator badge.

Left at its default, Swagger UI renders a badge by handing the address of this
API's document to ``validator.swagger.io``. On a node's network that is a request
that cannot succeed, and anywhere else it is the URL of an internal API sent to a
third party unasked. ``None`` becomes a JSON ``null``, which is how Swagger UI is
told there is no validator.
"""

TITLE = "Dusk API gateway"
DESCRIPTION = """
Run programs on dusk nodes over HTTP.

`POST /connect` with a node's host and port returns a **descriptor** - an
eight-digit code like `a3f91c07`. Pass it to `/sh` to run commands on that
node, and to `/disconnect` when you are done. Open as many connections as you
need; each gets its own descriptor.

`GET /help` lists the programs the gateway can run. It needs no connection.

AI agents can drive the same nodes through the MCP server at `/mcp`.
"""


class StrictModel(BaseModel):
    """Base for every request body: no type coercion, no unexpected fields.

    Strict mode is what keeps a request meaning exactly what it says. Without it
    pydantic reads ``{"port": "9090"}`` as the number 9090 and ``{"port": true}``
    as port 1, because ``bool`` is a subclass of ``int`` in Python. A gateway
    that guesses is a gateway that connects somewhere the caller did not ask for.
    """

    model_config = ConfigDict(strict=True, extra="forbid")


class ConnectRequest(StrictModel):
    host: str
    port: int


class ConnectResponse(BaseModel):
    descriptor: str


class DisconnectRequest(StrictModel):
    descriptor: str


class DisconnectResponse(BaseModel):
    descriptor: str


class ShellRequest(StrictModel):
    descriptor: str
    command: str


class ShellResponse(BaseModel):
    """What a program produced, one entry per value, in order.

    An entry is whatever the program returned: a string, a number, or a record
    of fields keyed by its type id. A program that produced nothing gives an
    empty list.
    """

    output: "list[Any]"


class Program(BaseModel):
    name: str
    version: str
    short_description: str
    long_description: str
    program_id: int


class ErrorResponse(BaseModel):
    error: str


MALFORMED = {
    "model": ErrorResponse,
    "description": "The request body is missing a field, has one of the wrong type, or carries one this endpoint does not take.",
}
NO_SUCH_CONNECTION = {
    "model": ErrorResponse,
    "description": "No open connection has that descriptor. It was never issued, or it has already been disconnected.",
}
NO_SUCH_PROGRAM = {
    "model": ErrorResponse,
    "description": "The gateway has no program by that name. `GET /help` lists the ones it does.",
}
NODE_UNREACHABLE = {
    "model": ErrorResponse,
    "description": "The node refused the connection or could not be reached.",
}
COMMAND_FAILED = {
    "model": ErrorResponse,
    "description": (
        "The node could not run the command. Among other reasons, a node runs "
        "each command in a shell task from a bounded pool, and answers `Busy` "
        "once they are all taken."
    ),
}


def build_application(
    registry: "ConnectionRegistry",
    programs: "list[dict[str, Any]]",
    owner: "Owner | None" = None,
) -> "FastAPI":
    """Build the REST application over ``registry`` and the node's ``programs``.

    Returned as an application of its own rather than as loose routes so that it
    carries its own OpenAPI document and renders its own errors as JSON, without
    changing how the MCP endpoint mounted beside it reports failures.
    """
    from . import REST_OWNER

    owner_of: "Owner" = owner or (lambda request: REST_OWNER)

    # docs_url and redoc_url are off because both of FastAPI's built-in pages
    # load their JavaScript and CSS from a CDN, which a node's operator may have
    # no route to. /docs below is the same Swagger UI served from this process.
    api = FastAPI(
        title=TITLE,
        description=DESCRIPTION,
        version="1",
        openapi_url="/openapi.json",
        docs_url=None,
        redoc_url=None,
    )
    api.mount("/static", StaticFiles(directory=STATIC), name="static")

    @api.get("/docs", include_in_schema=False)
    async def docs(request: Request) -> Response:
        """Swagger UI over this API, served entirely from this gateway."""
        # Read from the scope rather than from the app, the way FastAPI's own
        # docs route does: the app is mounted, so the prefix it is reachable
        # under is only known per request. Getting this wrong renders an empty
        # page, because every URL on it would be missing the /v1.
        mounted_at = request.scope.get("root_path", "").rstrip("/")
        return get_swagger_ui_html(
            openapi_url=f"{mounted_at}/openapi.json",
            title=f"{TITLE} - API reference",
            swagger_js_url=f"{mounted_at}/static/swagger-ui-bundle.js",
            swagger_css_url=f"{mounted_at}/static/swagger-ui.css",
            swagger_favicon_url=BLANK_FAVICON,
            swagger_ui_parameters=SWAGGER_UI_PARAMETERS,
        )

    @api.post(
        "/connect",
        summary="Open a connection to a node",
        description=(
            "Opens a connection to the dusk node at `host:port` and returns the "
            "descriptor that names it. Every call returns a new descriptor, so "
            "repeat connections to one node stay distinct."
        ),
        response_description="The descriptor for the new connection.",
        responses={400: MALFORMED, 502: NODE_UNREACHABLE},
    )
    async def connect(body: ConnectRequest, request: Request) -> ConnectResponse:
        request_owner = owner_of(request)
        try:
            descriptor, _ = await anyio.to_thread.run_sync(
                registry.connect, request_owner, body.host, body.port
            )
        except Exception as failure:
            # The node is the upstream this gateway fronts, so a node that will
            # not accept a connection is a bad gateway, not a bad request.
            raise HTTPException(
                502, f"cannot connect to {body.host}:{body.port}: {failure}"
            )
        return ConnectResponse(descriptor=descriptor)

    @api.post(
        "/disconnect",
        summary="Close a connection",
        description="Closes the connection the descriptor names. It is invalid afterward.",
        response_description="The descriptor that was closed.",
        responses={400: MALFORMED, 404: NO_SUCH_CONNECTION},
    )
    async def disconnect(
        body: DisconnectRequest, request: Request
    ) -> DisconnectResponse:
        request_owner = owner_of(request)
        try:
            await anyio.to_thread.run_sync(
                registry.disconnect, request_owner, body.descriptor
            )
        except KeyError:
            raise HTTPException(404, _unknown_descriptor(body.descriptor))
        return DisconnectResponse(descriptor=body.descriptor)

    @api.post(
        "/sh",
        summary="Run a shell command on a node",
        description=(
            "Runs one dusk shell command line on the connection the descriptor "
            "names - exactly what you would type at the `dusk` prompt - and "
            "returns everything the program produced. Blocks until the program "
            "finishes."
        ),
        response_model=ShellResponse,
        response_description="Everything the program produced.",
        responses={400: MALFORMED, 404: NO_SUCH_CONNECTION, 502: COMMAND_FAILED},
    )
    async def shell(body: ShellRequest, request: Request) -> Response:
        request_owner = owner_of(request)
        try:
            connection = registry.get(request_owner, body.descriptor)
        except KeyError:
            raise HTTPException(404, _unknown_descriptor(body.descriptor))
        try:
            output = [value async for value in _values(connection, body.command)]
        except Exception as failure:
            # Nothing has been sent yet, so unlike the streaming endpoint this
            # can still be a status code. Letting it escape instead would answer
            # a plain-text 500, the one response this API does not render as
            # JSON.
            raise HTTPException(502, f"the node could not run the command: {failure}")
        # Serialised here rather than through the response model: a program's
        # values reach JSON via ``default=str`` (see _render), which FastAPI's
        # encoder would not apply. The model above still documents the shape.
        return Response(
            json.dumps({"output": output}, default=str),
            media_type="application/json",
        )

    @api.post(
        "/sh/stream",
        summary="Run a shell command and stream its output",
        description=(
            "The same as `/sh`, but each value reaches you as the program "
            "produces it instead of all of them once it finishes, as a "
            "[Server-Sent Events](https://developer.mozilla.org/docs/Web/API/Server-sent_events) "
            "stream.\n\n"
            "Four kinds of event are sent:\n\n"
            "- `start` - the stream is live. Sent straight away, before the "
            "program has produced anything, so a quiet command is "
            "distinguishable from a gateway that never answered.\n"
            "- `output` - one value the program produced. Its `data` is that "
            "value as JSON, the same shape `/sh` puts in its `output` list.\n"
            "- `end` - the program finished. Nothing follows it.\n"
            "- `error` - the command failed partway through. Nothing follows "
            'it either, and its `data` is `{"error": "..."}`.\n\n'
            "Use this for a command that runs for a while or never ends on its "
            "own, such as following a node's logs. Disconnecting is how you "
            "stop reading; it does not stop the program, which keeps running on "
            "the node until you `kill` it."
        ),
        # response_class is what keeps application/json out of the document:
        # FastAPI adds it from the route's default otherwise, and this endpoint
        # never sends it.
        response_class=StreamingResponse,
        response_description="An event stream of the program's output.",
        responses={
            200: {"content": {"text/event-stream": {}}},
            400: MALFORMED,
            404: NO_SUCH_CONNECTION,
        },
    )
    async def shell_stream(body: ShellRequest, request: Request) -> Response:
        request_owner = owner_of(request)
        try:
            connection = registry.get(request_owner, body.descriptor)
        except KeyError:
            raise HTTPException(404, _unknown_descriptor(body.descriptor))
        return StreamingResponse(
            _events(connection, body.command),
            media_type="text/event-stream",
            headers={
                "cache-control": "no-cache",
                # Tells nginx not to buffer the response. Without it a proxy in
                # front of the gateway can hold every event until the program
                # finishes, which is the one thing this endpoint exists to avoid.
                "x-accel-buffering": "no",
            },
        )

    @api.get(
        "/help",
        summary="List the Gateway's available programs",
        description=(
            "Every program the gateway can run, with its help text. The set is "
            "compiled into the gateway, so this works before you connect to "
            "anything."
        ),
        response_description="Every program the gateway can run.",
    )
    async def help_all() -> list[Program]:
        return [Program(**program) for program in programs]

    @api.get(
        "/help/{program}",
        summary="Describe a program available on the gateway",
        description="The same entry `GET /help` returns, for one program by name.",
        response_description="The program's entry.",
        responses={404: NO_SUCH_PROGRAM},
    )
    async def help_one(program: str) -> Program:
        for entry in programs:
            if entry["name"] == program:
                return Program(**entry)
        raise HTTPException(404, f"no sh entry named {program!r}")

    api.add_exception_handler(HTTPException, _http_exception_as_json)
    api.add_exception_handler(RequestValidationError, _invalid_request_as_json)
    api.openapi = _openapi_without_the_unreachable_422(api)
    return api


def _openapi_without_the_unreachable_422(api: "FastAPI") -> "Callable[[], dict]":
    """Wrap ``api.openapi`` to drop the 422 FastAPI documents but this API never sends.

    FastAPI adds a ``422`` response to every operation that validates anything,
    because that is what it would return for a body pydantic rejects. This API
    reports those as ``400`` instead (see :func:`_invalid_request_as_json`), so
    leaving the ``422`` in the document would describe a status no caller can
    ever receive - and a spec that lies is worse than no spec. The schemas
    describing its body go with it, since nothing references them afterward.
    """
    generate = api.openapi

    def openapi() -> dict:
        schema = generate()
        for operations in schema.get("paths", {}).values():
            for operation in operations.values():
                operation.get("responses", {}).pop("422", None)
        schemas = schema.get("components", {}).get("schemas", {})
        for unreferenced in ("HTTPValidationError", "ValidationError"):
            schemas.pop(unreferenced, None)
        return schema

    return openapi


def _unknown_descriptor(descriptor: str) -> str:
    return f"unknown connection descriptor: {descriptor!r}"


async def _http_exception_as_json(request: Request, exception: Exception) -> Response:
    """Render an ``HTTPException`` as an :class:`ErrorResponse`.

    FastAPI's built-in handler answers ``{"detail": ...}``, which would make the
    error bodies disagree with the ``{"error": ...}`` this API documents.
    """
    assert isinstance(exception, HTTPException)
    return JSONResponse({"error": exception.detail}, status_code=exception.status_code)


async def _invalid_request_as_json(request: Request, exception: Exception) -> Response:
    """Render a body that failed validation as a ``400``, not FastAPI's ``422``.

    A malformed request body is a bad request, and reporting it as one keeps
    every failure of this API to a single status set and a single body shape.
    The per-field detail pydantic collected is flattened into the message so
    nothing a caller needs to fix the request is lost.
    """
    assert isinstance(exception, RequestValidationError)
    return JSONResponse({"error": _explain(exception)}, status_code=400)


def _explain(exception: RequestValidationError) -> str:
    """One human-readable line for everything wrong with a request body."""
    complaints = []
    for error in exception.errors():
        field = ".".join(str(part) for part in error["loc"] if part != "body")
        complaints.append(f"{field}: {error['msg']}" if field else error["msg"])
    return "; ".join(complaints) or "invalid request body"


async def _values(connection: "Connection", command: str) -> "AsyncIterator[Any]":
    """Every value a command produces, awaited rather than waited on.

    ``ShellOutput.next_value`` is awaitable, so a command that is quiet for an
    hour costs this gateway a task and nothing else. Draining it with the
    blocking ``__next__`` instead would park an OS thread for that hour, and the
    pool those come from is shared with every other request the gateway serves.
    """
    output = connection.sh(command)
    while True:
        try:
            value = await output.next_value()
        except StopAsyncIteration:
            return
        yield _renderable(value)


async def _events(connection: "Connection", command: str) -> "AsyncIterator[str]":
    """Server-Sent Events for one command's output, sent as it is produced."""
    # Sent before anything is read, so the headers reach the client immediately.
    # Otherwise a command that is quiet for ten minutes looks indistinguishable
    # from a gateway that never answered.
    yield _sse("start", {})
    try:
        async for value in _values(connection, command):
            yield _sse("output", value)
    except Exception as failure:
        # The response has already begun, so this cannot be a status code. It is
        # reported in the stream instead, which is why `error` is one of the
        # events the endpoint documents.
        yield _sse("error", {"error": str(failure)})
        return
    yield _sse("end", {})


def _sse(event: str, data: "Any") -> str:
    """One Server-Sent Event. JSON on a single line, so no value can split it."""
    return f"event: {event}\ndata: {json.dumps(data, default=str)}\n\n"


def _renderable(value: "Any") -> "Any":
    """``value`` if JSON can carry it, its ``repr`` if not. See :func:`_render`."""
    try:
        json.dumps(value, default=str)
    except TypeError, ValueError:
        return repr(value)
    return value


def _render(output: "Any") -> "list[Any]":
    """Drain a ``ShellOutput`` iterator into a list of JSON-ready values."""
    return [_renderable(item) for item in output]
