"""Tool registration for the dusk MCP server.

Registers the gateway ``connect`` / ``disconnect`` tools and one tool per
available dusk program. Program tools are enumerated from ``Dusk.help()`` (the
link-time program set), so they are known without any connection.
"""

from __future__ import annotations

import json
from typing import TYPE_CHECKING, cast

import anyio
import anyio.to_thread

# Imported at runtime (not only under TYPE_CHECKING) because FastMCP detects the
# context parameter by resolving the tool's type hints; a string annotation that
# can't resolve to the real Context class would be treated as a tool input.
from mcp.server.fastmcp import Context
from mcp.types import TASK_OPTIONAL, CallToolResult, TextContent, ToolExecution

if TYPE_CHECKING:
    from typing import Any

    from mcp.server.experimental.task_context import ServerTaskContext
    from mcp.server.fastmcp import FastMCP
    from mcp.types import ContentBlock, Tool

    from . import ConnectionRegistry


def register_tools(server: "FastMCP", registry: "ConnectionRegistry") -> None:
    """Register every tool onto ``server``."""
    from .. import Dusk

    async def connect(host: str, port: int, context: Context) -> str:
        # Async so the session-end hook is registered on the event loop that
        # owns the session; the blocking Dusk connect runs off it in a thread.
        session = context.session
        descriptor, is_first_connection = await anyio.to_thread.run_sync(
            registry.connect, session, host, port
        )
        if is_first_connection:

            async def disconnect_session_on_close() -> None:
                # Runs from BaseSession.__aexit__'s exit-stack unwind, which on
                # server shutdown happens while this task is already cancelled.
                # to_thread.run_sync issues a cancellation checkpoint, so without
                # the shield the CancelledError re-raises mid-unwind and corrupts
                # anyio's cancel-scope stack ("Attempted to exit a cancel scope
                # that isn't the current task's current cancel scope").
                with anyio.CancelScope(shield=True):
                    await anyio.to_thread.run_sync(registry.disconnect_session, session)

            session._exit_stack.push_async_callback(disconnect_session_on_close)
        return descriptor

    server.add_tool(
        connect,
        name="connect",
        title="Connect to a dusk server",
        description=(
            "Open a connection to a dusk server at the given host and port. "
            "Returns a descriptor string (formatted host:port#n) that "
            "identifies this connection; pass it to every program tool and to "
            "the disconnect tool. You may hold several connections at once — "
            "each call returns a new descriptor. Connections are closed "
            "automatically when this session ends, but call disconnect when "
            "you are done with one to free it sooner."
        ),
    )

    async def disconnect(descriptor: str, context: Context) -> str:
        await anyio.to_thread.run_sync(registry.disconnect, context.session, descriptor)
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

    programs = Dusk.help()
    for program in programs:
        _register_program_tool(server, registry, program)
    _register_task_support(server, frozenset(program["name"] for program in programs))


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

    async def run(descriptor: str, context: Context, arguments: str = "") -> str:
        connection = registry.get(context.session, descriptor)
        command = name if not arguments else f"{name} {arguments}"
        return await anyio.to_thread.run_sync(lambda: _drain(connection.sh(command)))

    server.add_tool(run, name=name, title=title, description=description)


def _register_task_support(server: "FastMCP", program_names: frozenset[str]) -> None:
    """Let the program tools run task-augmented (MCP tasks).

    A task-augmented call returns a task id immediately while the program runs
    in the background; the client polls the task and fetches the result when
    the program finishes, so a long-running program never stalls the caller.

    FastMCP has no hook for tasks — task support lives on the lowlevel server,
    and FastMCP's result conversion cannot carry a ``CreateTaskResult`` — so
    this re-registers the lowlevel handlers FastMCP installed: ``list_tools``
    to advertise ``execution.taskSupport "optional"`` on every program tool,
    and ``call_tool`` to divert task-augmented calls through ``run_task``.
    Plain calls and the gateway tools (connect / disconnect) behave as before;
    task-augmenting a gateway tool is rejected, per the MCP spec's handling of
    tools without task support.
    """
    lowlevel_server = server._mcp_server
    lowlevel_server.experimental.enable_tasks()

    async def list_tools_with_task_support() -> list[Tool]:
        tools = await server.list_tools()
        for tool in tools:
            if tool.name in program_names:
                tool.execution = ToolExecution(taskSupport=TASK_OPTIONAL)
        return tools

    async def call_tool_task_aware(name: str, arguments: dict[str, Any]):
        experimental = lowlevel_server.request_context.experimental
        experimental.validate_task_mode(
            TASK_OPTIONAL if name in program_names else None
        )
        if not experimental.is_task:
            return await server.call_tool(name, arguments)

        async def run_program_in_task(
            task_context: ServerTaskContext,
        ) -> CallToolResult:
            result = await server.call_tool(name, arguments)
            if isinstance(result, tuple):
                unstructured_content, structured_content = cast(
                    "tuple[list[ContentBlock], dict[str, Any]]", result
                )
            elif isinstance(result, dict):
                unstructured_content = [
                    TextContent(type="text", text=json.dumps(result, indent=2))
                ]
                structured_content = result
            else:
                unstructured_content = result
                structured_content = None
            return CallToolResult(
                content=list(unstructured_content),
                structuredContent=structured_content,
                isError=False,
            )

        return await experimental.run_task(run_program_in_task)

    lowlevel_server.list_tools()(list_tools_with_task_support)
    lowlevel_server.call_tool(validate_input=False)(call_tool_task_aware)


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
