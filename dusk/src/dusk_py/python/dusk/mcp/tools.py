"""Tool registration for the dusk MCP server.

Registers the gateway ``connect`` / ``disconnect`` tools and one tool per
available dusk program. Program tools are enumerated from ``Dusk.help()`` (the
link-time program set), so they are known without any connection.
"""

from __future__ import annotations

import json
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from mcp.server.fastmcp import FastMCP

    from . import ConnectionRegistry


def register_tools(server: "FastMCP", registry: "ConnectionRegistry") -> None:
    """Register every tool onto ``server``."""
    from .. import Dusk

    def connect(host: str, port: int) -> str:
        return registry.connect(host, port)

    server.add_tool(
        connect,
        name="connect",
        title="Connect to a dusk server",
        description=(
            "Open a connection to a dusk server at the given host and port. "
            "Returns a descriptor string (formatted host:port#n) that "
            "identifies this connection; pass it to every program tool and to "
            "the disconnect tool. You may hold several connections at once — "
            "each call returns a new descriptor."
        ),
    )

    def disconnect(descriptor: str) -> str:
        registry.disconnect(descriptor)
        return f"disconnected {descriptor}"

    server.add_tool(
        disconnect,
        name="disconnect",
        title="Disconnect from a dusk server",
        description=(
            "Close a dusk connection previously opened with the connect tool, "
            "identified by its descriptor. The descriptor is invalid afterward."
        ),
    )

    for program in Dusk.help():
        _register_program_tool(server, registry, program)


def _register_program_tool(
    server: "FastMCP", registry: "ConnectionRegistry", program: dict
) -> None:
    """Register one tool for a single dusk program.

    The program's argument grammar is not described by ``help``, so the tool
    takes a freeform ``arguments`` string the caller fills in by reading the
    description; the handler runs ``<name> <arguments>`` over the shell.
    """
    name = program["name"]
    short_description = program["short_description"]
    long_description = program["long_description"]

    title = f"Dusk `{name}`"

    usage = (
        f"Run the dusk `{name}` program on the Dusk Node available via the connection identified by "
        f"`descriptor` (from the connect tool). Put any program arguments in "
        f"the `arguments` string; they are appended after the program name."
    )
    description = f"""
    {usage}

    short program description:
    {short_description}
    long program description:
    {long_description}
    """

    def run(descriptor: str, arguments: str = "") -> str:
        connection = registry.get(descriptor)
        command = name if not arguments else f"{name} {arguments}"
        return _drain(connection.sh(command))

    server.add_tool(run, name=name, title=title, description=description)


def _drain(output) -> str:
    """Collect a ``ShellOutput`` iterator into a single text result.

    Each yielded object is rendered as JSON where possible, falling back to
    ``repr`` for objects JSON cannot serialize. One object per line.
    """
    rendered = []
    for item in output:
        try:
            rendered.append(json.dumps(item, default=str))
        except (TypeError, ValueError):
            rendered.append(repr(item))
    return "\n".join(rendered)
