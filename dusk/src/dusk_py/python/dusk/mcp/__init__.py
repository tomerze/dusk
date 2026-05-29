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

import threading
from typing import TYPE_CHECKING

import uvicorn
from mcp.server.fastmcp import FastMCP

from .tools import register_tools

if TYPE_CHECKING:
    from .. import Dusk


class ConnectionRegistry:
    """Thread-safe map of descriptor -> ``Dusk`` connection.

    FastMCP dispatches tool calls on worker threads, so every access to the
    map is guarded by a lock. Descriptors are human-readable: ``host:port#n``,
    where ``n`` is a per-registry counter that keeps repeat connections to the
    same address distinct.
    """

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._connections: dict[str, Dusk] = {}
        self._counter = 0

    def connect(self, host: str, port: int) -> str:
        """Open a connection and return its descriptor."""
        from .. import Dusk

        connection = Dusk(host, port)
        with self._lock:
            self._counter += 1
            descriptor = f"{host}:{port}#{self._counter}"
            self._connections[descriptor] = connection
        return descriptor

    def get(self, descriptor: str) -> Dusk:
        """Look up a connection, raising ``KeyError`` if unknown."""
        with self._lock:
            connection = self._connections.get(descriptor)
        if connection is None:
            raise KeyError(f"unknown connection descriptor: {descriptor!r}")
        return connection

    def disconnect(self, descriptor: str) -> None:
        """Close and forget a connection, raising ``KeyError`` if unknown."""
        with self._lock:
            connection = self._connections.pop(descriptor, None)
        if connection is None:
            raise KeyError(f"unknown connection descriptor: {descriptor!r}")
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
    """
    registry = ConnectionRegistry()

    server = FastMCP(
        "dusk",
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
    return server.streamable_http_app()


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
