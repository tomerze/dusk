"""The dusk API gateway: a REST API over dusk nodes, with MCP on top.

Run it with ``dusk.gw.serve(ip, port)`` (or build the ASGI app with :func:`app`
and serve it yourself). The gateway is what holds the connections: a client asks
it to open one to a node with ``connect``, the gateway opens it and keeps it in
its :class:`ConnectionRegistry`, and hands back a descriptor naming it. Every
later call names that descriptor, and the gateway does the talking to the node.
What the gateway does *not* have is a connection of its own before a client asks
for one, or any notion of a single node it belongs to.

One process serves two protocols over the same connection registry:

- the **REST API** under ``/v1`` (see :mod:`dusk.gw.rest`), which mirrors the
  ``Dusk`` Python class one endpoint per method;
- the **MCP server** at ``/mcp`` (see :mod:`dusk.gw.mcp`), which exposes each
  dusk program as an MCP tool. It is always mounted; there is no switch to turn
  it off.

A descriptor minted over REST is usable over REST only, and one minted by an MCP
session is usable by that session only - the registry keys connections by the
owner that opened them so it can tear them all down when that owner goes away.

The ``mcp``, ``starlette`` and ``uvicorn`` packages are regular dependencies of
the wheel, imported at this module's top. The package is only imported on
demand, so plain ``import dusk`` does not load them.
"""

from __future__ import annotations

import contextlib
import os
import secrets
import threading
from typing import TYPE_CHECKING

import anyio
import anyio.to_thread
import uvicorn
from starlette.routing import Mount

from .mcp import build_application as build_mcp_application
from .rest import build_application as build_rest_application

if TYPE_CHECKING:
    from typing import Any, Protocol

    from starlette.applications import Starlette

    class Output(Protocol):
        """What ``sh`` hands back: values to await, one at a time.

        ``next_value`` resolves to the command's next value and raises
        ``StopAsyncIteration`` once it has finished. The gateway awaits it rather
        than iterating the same object synchronously, so a command that produces
        nothing for an hour costs a task and not an OS thread.
        """

        async def next_value(self) -> Any: ...

    class Connection(Protocol):
        """The part of ``Dusk`` the gateway uses: run a command, hang up."""

        def sh(self, command: str) -> Output: ...
        def disconnect(self) -> None: ...

    class ConnectionFactory(Protocol):
        """Opens a :class:`Connection` to the node at ``host:port``. ``Dusk`` itself."""

        def __call__(self, host: str, port: int) -> Connection: ...


REST_OWNER = "rest"
"""The owner every REST-minted connection belongs to.

REST is a single shared owner because the API is stateless above the descriptor:
there is nothing on an HTTP request that identifies a longer-lived client, so
every REST caller draws from the same pool of descriptors. MCP sessions each own
their connections instead, and cannot see REST's or each other's.
"""


class ConnectionRegistry:
    """Thread-safe map of owner -> {descriptor -> connection}.

    An owner is whatever opened the connection: an MCP session object, or
    :data:`REST_OWNER` for the REST API. Grouping by owner is what lets every
    connection an owner opened be torn down when that owner goes away (see
    :meth:`disconnect_owner`) and when the whole server shuts down (see
    :meth:`disconnect_all`).

    Handlers offload the blocking dusk calls to worker threads, so every access
    to the map is guarded by a lock. The blocking ``disconnect`` is always called
    outside the lock, so a slow connection teardown never blocks other calls.
    A descriptor is eight hexadecimal digits (see :meth:`mint`); lookups are
    scoped to the owner that holds it.

    ``connection_factory`` is the dial for what a connection *is*: it defaults to
    the native ``Dusk`` class, and a caller that wants to drive the gateway
    against something else - a test double, an instrumented client - passes its
    own.
    """

    def __init__(self, connection_factory: "ConnectionFactory | None" = None) -> None:
        self._lock = threading.Lock()
        self._connections: dict[object, dict[str, Connection]] = {}
        self._connection_factory = connection_factory

    def connect(self, owner: object, host: str, port: int) -> tuple[str, bool]:
        """Open a connection for ``owner`` and return its descriptor.

        The returned flag is ``True`` when this is the first connection for
        ``owner``, so the caller can register an owner-end teardown hook exactly
        once. REST has no owner-end event and ignores it.
        """
        connection_factory = self._connection_factory
        if connection_factory is None:
            from .. import Dusk

            connection_factory = Dusk

        connection = connection_factory(host, port)
        with self._lock:
            descriptor = self.mint()
            owner_connections = self._connections.get(owner)
            is_first_connection = owner_connections is None
            if owner_connections is None:
                owner_connections = {}
                self._connections[owner] = owner_connections
            owner_connections[descriptor] = connection
        return descriptor, is_first_connection

    def mint(self) -> str:
        """A descriptor no connection in this registry holds. Call under the lock.

        Eight hexadecimal digits, drawn from ``secrets`` rather than counted up.
        Descriptors are not a security boundary - every REST caller shares one
        owner and so may use any REST descriptor - but a counter would publish
        how many connections the gateway has opened and let any caller address
        another's connection by typing the number below their own, which is a
        worse failure to leave lying around than 32 bits of randomness.

        Retried until unused, because 32 bits is small enough for a collision to
        be an event that happens rather than one that is argued away: at a
        thousand concurrent connections the chance of a birthday collision is
        about one in ten thousand, and reusing a live descriptor would hand one
        caller another's node.
        """
        while True:
            descriptor = secrets.token_hex(4)
            if not any(
                descriptor in owner_connections
                for owner_connections in self._connections.values()
            ):
                return descriptor

    def get(self, owner: object, descriptor: str) -> "Connection":
        """Look up a connection held by ``owner``, raising ``KeyError`` if unknown."""
        with self._lock:
            owner_connections = self._connections.get(owner)
            connection = (
                owner_connections.get(descriptor) if owner_connections else None
            )
        if connection is None:
            raise KeyError(f"unknown connection descriptor: {descriptor!r}")
        return connection

    def disconnect(self, owner: object, descriptor: str) -> None:
        """Close and forget a connection held by ``owner``, raising ``KeyError`` if unknown."""
        with self._lock:
            owner_connections = self._connections.get(owner)
            connection = (
                owner_connections.pop(descriptor, None) if owner_connections else None
            )
        if connection is None:
            raise KeyError(f"unknown connection descriptor: {descriptor!r}")
        connection.disconnect()

    def disconnect_owner(self, owner: object) -> None:
        """Close and forget every connection ``owner`` opened. No-op if it opened none."""
        with self._lock:
            owner_connections = self._connections.pop(owner, None)
        if not owner_connections:
            return
        for connection in owner_connections.values():
            connection.disconnect()

    def disconnect_all(self) -> None:
        """Close and forget every connection across all owners. Called on server shutdown."""
        with self._lock:
            all_owner_connections = list(self._connections.values())
            self._connections.clear()
        for owner_connections in all_owner_connections:
            for connection in owner_connections.values():
                connection.disconnect()


def app(
    ip: str = "127.0.0.1",
    connection_factory: "ConnectionFactory | None" = None,
    programs: "list[dict[str, Any]] | None" = None,
) -> "Starlette":
    """Build the gateway's ASGI application: REST under ``/v1``, MCP at ``/mcp``.

    ``ip`` is the address the returned app will be served on. It changes no
    binding - that is the ASGI server's job - but the MCP endpoint refuses
    requests whose Host header disagrees with a loopback ``ip``, so an app served
    somewhere other than where it was told would turn every MCP client away with
    ``421 Misdirected Request``. :func:`serve` passes the address it binds. Pass
    the same address you will serve on.

    Hand the returned app to your own ASGI server when you want full control
    over the run parameters::

        uvicorn.run(dusk.gw.app("0.0.0.0"), host="0.0.0.0", port=9100,
                    ssl_keyfile=..., log_config=...)

    :func:`serve` is the batteries-included wrapper that runs uvicorn for you.

    Neither surface is served from the bare root: ``/`` returns 404. REST clients
    call ``http://<host>:<port>/v1/…`` and MCP clients connect to
    ``http://<host>:<port>/mcp``.

    The other two parameters are dials for callers not driving real nodes.
    ``connection_factory`` replaces the native ``Dusk`` class the registry opens
    connections with, and ``programs`` replaces the program set that ``/v1/help``
    reports and that the MCP tools are generated from - the list ``Dusk.help()``
    returns, each entry a dict with ``name``, ``short_description`` and
    ``long_description``. Left unset, both come from the linked dusk impl.

    Note: the connection registry is in-process state, so serve a single worker.
    Multiple worker processes would each hold a separate, unshared registry, so a
    descriptor from one worker would be unknown to another.

    Connections are torn down automatically: when an MCP client session ends (a
    session-end hook calls :meth:`ConnectionRegistry.disconnect_owner`), and when
    the server itself shuts down (the app lifespan below calls
    :meth:`ConnectionRegistry.disconnect_all`). A forgotten ``disconnect`` leaks a
    connection only until its owner goes away - and a REST descriptor, whose
    owner is the process, until the gateway stops.
    """
    # The gateway has no terminal to give away, prevents a rogue model from calling `logs view` for example.
    os.environ["DUSK_NON_INTERACTIVE"] = "1"
    registry = ConnectionRegistry(connection_factory)

    if programs is None:
        from .. import Dusk

        program_set: "list[dict[str, Any]]" = Dusk.help()
    else:
        program_set = programs

    application = build_mcp_application(registry, program_set, ip)
    application.routes.append(
        Mount("/v1", build_rest_application(registry, program_set))
    )

    # FastMCP builds the app with its own lifespan (the streamable-HTTP session
    # manager), so on_shutdown handlers are ignored. Wrap that lifespan to
    # disconnect every remaining dusk connection when the server stops, off the
    # event loop since disconnect blocks joining the connection thread.
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
                # shield in mcp.py disconnect_owner_on_close.
                with anyio.CancelScope(shield=True):
                    await anyio.to_thread.run_sync(registry.disconnect_all)

    application.router.lifespan_context = lifespan_disconnecting_all
    return application


def serve(ip: str, port: int) -> None:
    """Run the gateway on ``ip:port``. Blocks.

    Serves :func:`app` through uvicorn explicitly, rather than FastMCP.run (which
    spins up its own uvicorn), so the HTTP server and its bind address are ours
    to control. For other uvicorn parameters, call :func:`app` and run uvicorn
    yourself.

    The REST API is under ``/v1`` and the MCP streamable-HTTP endpoint is at
    ``/mcp``; the bare ``ip:port`` returns 404.
    """
    uvicorn.run(app(ip), host=ip, port=port)
