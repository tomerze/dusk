from __future__ import annotations

import asyncio
import contextlib
import logging
import threading
from collections.abc import Callable
from typing import TYPE_CHECKING, Any

from dusk.gw.mcp import INSTRUCTIONS, close_with_session
from dusk.gw.rest import MALFORMED, NODE_UNREACHABLE, ConnectResponse, ErrorResponse
from fastapi import FastAPI, HTTPException, Request
from mcp.server.fastmcp import Context, FastMCP
from pydantic import ValidationError

from .auth import PRINCIPAL, Principal
from .models import ConnectRequest, NodeRef
from .nodes import Connection, SessionsExhausted, Unreachable, connect, described
from .work import classify, release

if TYPE_CHECKING:
    from dusk.gw import ConnectionRegistry

    from .api import Services

logger = logging.getLogger(__name__)

STEP_ONE = (
    "1. Call the `connect` tool with a Node's host and port to open a connection."
)
DAWN_STEP_ONE = (
    "1. Call the `connect` tool with the node reference (device_id, installation_id, "
    "namespace_id, and nightfall only when you were given one) and the pid that was "
    "issued for this session, to open a connection."
)
if STEP_ONE not in INSTRUCTIONS:
    raise RuntimeError(
        "dusk.gw's MCP instructions no longer describe connecting in step 1"
    )
DAWN_INSTRUCTIONS = INSTRUCTIONS.replace(STEP_ONE, DAWN_STEP_ONE)
NIGHTFALL_REFUSED = {
    "model": ErrorResponse,
    "description": "nightfall refused the session.",
}
SESSION_CONFLICT = {
    "model": ErrorResponse,
    "description": "This pid already has an open session.",
}
TOO_SLOW = {"model": ErrorResponse, "description": "The node did not answer in time."}
AT_CAPACITY = {
    "model": ErrorResponse,
    "description": "dawn holds as many node sessions, interactive sessions or "
    "connections being opened as it may, or is shutting down.",
}


class InteractiveSession:
    def __init__(
        self,
        connection: Connection,
        node: NodeRef,
        pid: int,
        services: Services,
        loop: asyncio.AbstractEventLoop,
    ) -> None:
        self._connection = connection
        self._node = node
        self._pid = pid
        self._services = services
        self._loop = loop
        self._loop_thread = threading.get_ident()
        self._lock = threading.Lock()
        self._closed = False
        self._ran = False
        self._attempt = services.results.attempt(node, pid, None, None, "interactive")
        self._expiry: asyncio.TimerHandle | None = None

    def expire(
        self, registry: ConnectionRegistry, owner: object, descriptor: str
    ) -> None:
        seconds = self._services.settings.limits.interactive_session_seconds

        def expired() -> None:
            logger.info(
                "closing an interactive session at the end of its lifetime",
                extra={**self._attempt.log(), "seconds": seconds},
            )
            with contextlib.suppress(KeyError):
                registry.disconnect(owner, descriptor)

        self._expiry = self._loop.call_later(seconds, expired)

    def sh(self, command: str) -> Any:
        self._ran = True
        return self._connection.sh(command)

    def disconnect(self) -> None:
        with self._lock:
            if self._closed:
                return
            self._closed = True
        future = asyncio.run_coroutine_threadsafe(self._close(), self._loop)
        if threading.get_ident() == self._loop_thread:
            return
        try:
            future.result(timeout=30)
        except Exception as failure:
            logger.warning(
                "an interactive session did not close cleanly",
                extra={**self._attempt.log(), "error": described(failure)},
            )

    async def _close(self) -> None:
        if self._expiry is not None:
            self._expiry.cancel()
        log = self._attempt.log()
        await self._connection.close()
        self._services.sessions.release()
        self._services.interactive.discard(session_key(self._node, self._pid))
        self._attempt.status = "succeeded"
        self._attempt.delivered = True
        await self._services.results.finished(self._attempt)
        logger.info(
            "closed an interactive session", extra={**log, "ran_commands": self._ran}
        )
        await release(
            self._services.connector,
            self._services.settings,
            self._node,
            [self._pid],
            log,
        )


def session_key(node: NodeRef, pid: int) -> tuple[str, str, int]:
    return (node.device_id, node.installation_id, pid)


async def open_session(
    services: Services, principal: Principal, node: NodeRef, pid: int
) -> InteractiveSession:
    services.check_address(node)
    services.check_accepting()
    key = session_key(node, pid)
    if key in services.interactive:
        raise HTTPException(409, f"pid {pid} already has an open session")
    limit = services.settings.limits.max_interactive_sessions
    if len(services.interactive) >= limit:
        raise HTTPException(
            503,
            f"dawn holds {limit} interactive sessions, its limit of "
            "limits.max_interactive_sessions",
        )
    try:
        services.sessions.reserve()
    except SessionsExhausted as failure:
        raise HTTPException(503, str(failure)) from failure
    services.interactive.add(key)
    attempt = services.results.attempt(node, pid, None, None, "interactive")
    try:
        connection = await connect(services.connector, services.settings, node, pid)
    except asyncio.CancelledError:
        services.sessions.release()
        services.interactive.discard(key)
        attempt.error = "dawn stopped before the session opened"
        await services.results.finished_despite_cancellation(attempt)
        raise
    except Exception as failure:
        services.sessions.release()
        services.interactive.discard(key)
        attempt.status = classify(failure, delivered=False)
        attempt.error = described(failure)
        await services.results.finished(attempt)
        raise refusal(failure) from failure
    logger.info(
        "opened an interactive session",
        extra={**attempt.log(), "principal": principal.subject},
    )
    return InteractiveSession(
        connection, node, pid, services, asyncio.get_running_loop()
    )


def refusal(failure: BaseException) -> HTTPException:
    if isinstance(failure, Unreachable):
        return HTTPException(502, str(failure))
    if isinstance(failure, SessionsExhausted):
        return HTTPException(503, str(failure))
    status = classify(failure, delivered=False)
    if status == "timed_out":
        return HTTPException(504, f"the node did not answer in time: {failure}")
    if status == "denied":
        return HTTPException(403, f"nightfall refused the session: {failure}")
    if status == "unreachable":
        return HTTPException(502, str(failure))
    return HTTPException(502, f"the node could not answer: {failure}")


def principal_of(request: Request) -> Principal:
    return request.state.principal


def owner_of(request: Request) -> object:
    principal = principal_of(request)
    return (principal.method, principal.subject)


def connect_route(
    services: Services,
) -> Callable[[FastAPI, ConnectionRegistry, Callable[[Request], object]], None]:
    def add(
        api: FastAPI, registry: ConnectionRegistry, owner: Callable[[Request], object]
    ) -> None:
        @api.post(
            "/connect",
            summary="Open an interactive session on a node",
            description=(
                "Opens a session on the node the reference names, through nightfall, in "
                "a shell at the pid issued for it, and returns the descriptor that names "
                "it. Pass the descriptor to `/sh` and `/disconnect`."
            ),
            response_description="The descriptor for the new session.",
            responses={
                400: MALFORMED,
                403: NIGHTFALL_REFUSED,
                409: SESSION_CONFLICT,
                502: NODE_UNREACHABLE,
                503: AT_CAPACITY,
            },
        )
        async def connect(body: ConnectRequest, request: Request) -> ConnectResponse:
            session = await open_session(
                services, principal_of(request), body.node, int(body.pid)
            )
            descriptor, _ = registry.register(owner(request), session)
            session.expire(registry, owner(request), descriptor)
            return ConnectResponse(descriptor=descriptor)

    return add


def connect_tool(services: Services) -> Callable[[FastMCP, ConnectionRegistry], None]:
    def add(server: FastMCP, registry: ConnectionRegistry) -> None:
        async def connect(
            device_id: str,
            installation_id: str,
            namespace_id: str,
            pid: str,
            context: Context,
            nightfall: str | None = None,
        ) -> str:
            request = context.request_context.request
            principal = (
                request.scope["state"][PRINCIPAL] if request is not None else None
            )
            if not isinstance(principal, Principal):
                raise ValueError("this MCP session has no authenticated principal")
            try:
                connection_request = ConnectRequest.model_validate(
                    {
                        "node": {
                            "device_id": device_id,
                            "installation_id": installation_id,
                            "namespace_id": namespace_id,
                            "nightfall": nightfall,
                        },
                        "pid": pid,
                    }
                )
            except ValidationError as failure:
                raise ValueError(str(failure)) from failure
            try:
                session = await open_session(
                    services,
                    principal,
                    connection_request.node,
                    int(connection_request.pid),
                )
            except HTTPException as failure:
                raise ValueError(failure.detail) from failure
            descriptor, first = registry.register(context.session, session)
            session.expire(registry, context.session, descriptor)
            if first:
                close_with_session(registry, context.session)
            return descriptor

        server.add_tool(
            connect,
            name="connect",
            title="Open a session on a dusk node",
            description=(
                "Open a session on one dusk node through nightfall, in a shell at the pid "
                "that was issued for it, as a decimal string. Pass the node reference you "
                "were given: device_id, installation_id, namespace_id, and nightfall only "
                "when it was part of the reference. Returns a descriptor - eight "
                "hexadecimal digits - to pass to every program tool and to the disconnect "
                "tool."
            ),
        )

    return add
