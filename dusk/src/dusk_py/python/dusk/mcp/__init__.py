"""MCP server exposing a dusk server's shell commands as tools.

Run it with ``dusk.mcp.serve(ip, port)`` (or build the ASGI app with
:func:`app` and serve it yourself). This is a *gateway* server: it holds no
dusk connection of its own. A connected MCP client opens connections with the
``connect`` tool, receives a human-readable descriptor, and reuses that
descriptor on the per-program tools and on ``disconnect``.

The ``mcp`` SDK and ``uvicorn`` are regular dependencies of the wheel, imported
at this module's top. The package is only imported on demand, so plain
``import dusk`` does not load them.
"""

from __future__ import annotations

import contextlib
import threading
from typing import TYPE_CHECKING

import anyio
import anyio.to_thread
import uvicorn
from mcp.server.fastmcp import FastMCP

from .tools import register_tools

if TYPE_CHECKING:
    from .. import Dusk


class ConnectionRegistry:
    """Thread-safe map of MCP session -> {descriptor -> ``Dusk`` connection}.

    Connections are grouped by the MCP session that opened them, so they can
    all be torn down when that session ends (see :meth:`disconnect_session`)
    and when the whole server shuts down (see :meth:`disconnect_all`).

    FastMCP dispatches tool calls on worker threads, so every access to the
    map is guarded by a lock. The blocking ``Dusk.disconnect`` is always called
    outside the lock, so a slow connection teardown never blocks other tool
    calls. Descriptors are human-readable: ``host:port#n``, where ``n`` is a
    per-registry counter that keeps repeat connections to the same address
    distinct; lookups are scoped to the session that owns the descriptor.
    """

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._connections: dict[object, dict[str, Dusk]] = {}
        self._counter = 0

    def connect(self, session: object, host: str, port: int) -> tuple[str, bool]:
        """Open a connection for ``session`` and return its descriptor.

        The returned flag is ``True`` when this is the first connection for
        ``session``, so the caller can register the session-end teardown hook
        exactly once.
        """
        from .. import Dusk

        connection = Dusk(host, port)
        with self._lock:
            self._counter += 1
            descriptor = f"{host}:{port}#{self._counter}"
            session_connections = self._connections.get(session)
            is_first_connection = session_connections is None
            if session_connections is None:
                session_connections = {}
                self._connections[session] = session_connections
            session_connections[descriptor] = connection
        return descriptor, is_first_connection

    def get(self, session: object, descriptor: str) -> Dusk:
        """Look up a connection owned by ``session``, raising ``KeyError`` if unknown."""
        with self._lock:
            session_connections = self._connections.get(session)
            connection = (
                session_connections.get(descriptor) if session_connections else None
            )
        if connection is None:
            raise KeyError(f"unknown connection descriptor: {descriptor!r}")
        return connection

    def disconnect(self, session: object, descriptor: str) -> None:
        """Close and forget a connection owned by ``session``, raising ``KeyError`` if unknown."""
        with self._lock:
            session_connections = self._connections.get(session)
            connection = (
                session_connections.pop(descriptor, None)
                if session_connections
                else None
            )
        if connection is None:
            raise KeyError(f"unknown connection descriptor: {descriptor!r}")
        connection.disconnect()

    def disconnect_session(self, session: object) -> None:
        """Close and forget every connection ``session`` opened. No-op if it opened none."""
        with self._lock:
            session_connections = self._connections.pop(session, None)
        if not session_connections:
            return
        for connection in session_connections.values():
            connection.disconnect()

    def disconnect_all(self) -> None:
        """Close and forget every connection across all sessions. Called on server shutdown."""
        with self._lock:
            all_session_connections = list(self._connections.values())
            self._connections.clear()
        for session_connections in all_session_connections:
            for connection in session_connections.values():
                connection.disconnect()


def app():
    """Build the dusk MCP server's streamable-HTTP ASGI application.

    Hand the returned app to your own ASGI server when you want full control
    over the run parameters::

        uvicorn.run(dusk.mcp.app(), host="0.0.0.0", port=9100,
                    ssl_keyfile=..., log_config=...)

    :func:`serve` is the batteries-included wrapper that runs uvicorn for you.

    The app serves the streamable-HTTP endpoint at the ``/mcp`` path (FastMCP's
    default), so MCP clients must connect to ``http://<host>:<port>/mcp`` — the
    bare host:port returns 404.

    Note: the connection registry is in-process state, so serve a single
    worker. Multiple worker processes would each hold a separate, unshared
    registry, so a descriptor from one worker would be unknown to another.

    Connections are torn down automatically: when an MCP client session ends
    (a session-end hook calls :meth:`ConnectionRegistry.disconnect_session`),
    and when the server itself shuts down (the app lifespan below calls
    :meth:`ConnectionRegistry.disconnect_all`). A forgotten ``disconnect`` tool
    call therefore leaks a connection only until its session closes.
    """
    registry = ConnectionRegistry()

    server = FastMCP(
        "Dusk",
        instructions=(
            """
Dusk is a framework for fleet management. A Dusk Node is controlled via a Dusk Client
A Dusk Client exposes a shell interface which allows running Dusk programs on the Node.
Dusk is not an OS itself but the interface to control a Node is
similar in nature to something like SSH: 
You connect to a Node and then run commands on it. 

**This MCP server is a gateway that exposes the Dusk Client's shell interface as MCP tools.**

This MCP server is a Dusk Client gateway: it opens client connections to Dusk Nodes on your 
behalf and exposes each Node program as a tool. Workflow:
1. Call the `connect` tool with a Node's host and port to open a connection. It returns a 
descriptor (a connection handle, formatted host:port#n) that identifies that one connection.
2. Pass that descriptor to the per-program tools (one tool per Dusk program) to run commands
 on that Node and read their output.
3. Call the `disconnect` tool with the descriptor when finished.

You may hold several connections to different Nodes at once, each identified by its own descriptor.
            """
        ),
    )
    register_tools(server, registry)

    application = server.streamable_http_app()

    # FastMCP builds the app with its own lifespan (the streamable-HTTP session
    # manager), so on_shutdown handlers are ignored. Wrap that lifespan to
    # disconnect every remaining Dusk connection when the server stops, off the
    # event loop since Dusk.disconnect blocks joining the connection thread.
    wrapped_lifespan_context = application.router.lifespan_context

    @contextlib.asynccontextmanager
    async def lifespan_disconnecting_all(starlette_app):
        async with wrapped_lifespan_context(starlette_app):
            try:
                yield
            finally:
                # Shielded: this runs during lifespan shutdown, which may already
                # be cancelled. to_thread.run_sync is a cancellation checkpoint,
                # and an unshielded CancelledError here would tear down the
                # wrapped session-manager lifespan mid-unwind. See the matching
                # shield in tools.py disconnect_session_on_close.
                with anyio.CancelScope(shield=True):
                    await anyio.to_thread.run_sync(registry.disconnect_all)

    application.router.lifespan_context = lifespan_disconnecting_all
    return application


def serve(ip: str, port: int) -> None:
    """Run the MCP server on ``ip:port`` over streamable HTTP. Blocks.

    Serves :func:`app` through uvicorn explicitly, rather than FastMCP.run
    (which spins up its own uvicorn), so the HTTP server and its bind address
    are ours to control. For other uvicorn parameters, call :func:`app` and run
    uvicorn yourself.

    The streamable-HTTP endpoint is at the ``/mcp`` path, so MCP clients connect
    to ``http://<ip>:<port>/mcp`` — the bare host:port returns 404.
    """
    uvicorn.run(app(), host=ip, port=port)
