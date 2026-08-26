"""The gateway's MCP server, served at ``/mcp`` over streamable HTTP.

Builds the FastMCP application and registers its tools: the gateway's ``connect``
/ ``disconnect``, and one tool per dusk program. Program tools come from the
program set the gateway was built with — the link-time set ``Dusk.help()``
reports — so they are known without any connection.

MCP is always mounted. The gateway serves it beside the ``/v1`` REST API
(:mod:`dusk.gw.rest`) out of the same connection registry, and there is no switch
to turn it off.
"""

from __future__ import annotations

import json
from typing import TYPE_CHECKING, cast

import anyio
import anyio.to_thread

# Imported at runtime (not only under TYPE_CHECKING) because FastMCP detects the
# context parameter by resolving the tool's type hints; a string annotation that
# can't resolve to the real Context class would be treated as a tool input.
from mcp.server.fastmcp import Context, FastMCP
from mcp.types import TASK_OPTIONAL, CallToolResult, TextContent, ToolExecution

if TYPE_CHECKING:
    from typing import Any

    from mcp.server.experimental.task_context import ServerTaskContext
    from mcp.types import ContentBlock, Tool
    from starlette.applications import Starlette

    from . import ConnectionRegistry

INSTRUCTIONS = """
Think of each Dusk Node as its own self-contained operating system — not a Linux box.
A Node has its own shell language (the Dusk shell), its own programs, and its own
process model (running processes you can list with `ps` and signal with `kill`). It is
NOT Unix: there is no `/bin`, no coreutils, no filesystem to shell out to, and no
Linux/Unix tools. The ONLY commands a Node can run are the Dusk programs compiled into
it — and this gateway exposes exactly those, one MCP tool per program (`ps`, `kill`,
`logs`, `hostname`, `sh`, `sleep`, …). If a capability isn't one of these program
tools, it does not exist on the Node: use the Dusk program built for the job (e.g.
`logs` to read logs, `ps` to list processes), never reach for a Unix command like
`cat`, `ls`, or `grep`. `sh` runs Dusk programs by name in the Dusk shell language; it
does not run Unix shell commands.

Dusk is a framework for fleet management: many such Nodes, each driven the same way.
You drive a Node a bit like SSH — connect, then run its programs — but what you reach
on the far side is a Dusk OS, not a Unix host.

**This MCP server is a gateway that opens client connections to Dusk Nodes on your
behalf and exposes each Node's programs as MCP tools.** Workflow:
1. Call the `connect` tool with a Node's host and port to open a connection. It returns a
descriptor (a connection handle: eight hexadecimal digits, for example a3f91c07) that
identifies that one connection.
2. Pass that descriptor to the per-program tools (one tool per Dusk program) to run that
Node's programs and read their output.
3. Call the `disconnect` tool with the descriptor when finished.

You may hold several connections to different Nodes at once, each identified by its own descriptor.

Reading program output:
A program's output is returned as JSON. Sometimes — not always — a program wraps its
result in a type id. When it does, the output is a JSON object with exactly one top-level
key: a `0x`-prefixed hexadecimal number (for example `0xcef2c7c974bf44ec`). That key is a
Cap'n Proto type id — a constant identifying *what kind of result this is*. It is NOT a node
id, connection descriptor, pid, namespace, session, or any runtime/per-call identifier, and
it carries no meaning beyond "the value underneath is of this type". The real data is the
object nested under that key.

Other programs return a plain JSON value instead — a string, object, number, boolean, list, or null —
with no type-id key, in which case the value itself is the data. So before interpreting any
output, check its shape: a single `0x…` key means "typed result, read the fields underneath";
anything else is the data directly. Never invent a meaning for the hex key or present it as data.

Long-running programs:
Every program tool supports task-augmented invocation (MCP tasks). If a command may run for a
while — a long `sleep`, a shell script, a live `logs` follow — invoke the tool *as an MCP task*
(this is NOT your client's generic "run in background" flag, which is a different mechanism and
will just block the call): you get a task id back immediately while the program runs on the
gateway, and you can keep working. Poll the task and fetch its result when the program finishes;
a program that never finishes you simply never poll. Quick commands work as plain synchronous
calls. Cancelling a task does NOT kill the program on the node — use the `kill` tool for that.

Commands on one connection run concurrently: a long-running one (a live `logs` follow you left
running as a task) does NOT block other commands on the same descriptor. Still prefer a bounded
read over an endless stream (see Reading logs).

Reading logs:
Use the `logs` tool's `dump` subcommand — it returns the logs to you as the tool result, one
structured record per log entry. For a BOUNDED snapshot that returns immediately, run the `logs`
tool with arguments `dump --replay-only`. To follow the live logs, run `dump` (without
--replay-only) as an MCP task and `kill` it when you're done — a plain follow never returns on its
own. Add `-l <level>` (error|warn|info|debug|trace) to raise the severity floor.

Do NOT use `logs view` (the interactive terminal pager; it is unavailable here — as is `sh`
with no arguments, the interactive shell) or `logs stream <url>` to read logs back: a stream's
url sink (file://, otlp://, http://) is written on THIS gateway host, not delivered to you, and a
plain stream never returns. Only `dump` hands the logs to you.

Each dumped entry is a typed record — read it as described in "Reading program output" above (the
single `0x…` key is the entry's Cap'n Proto type id; the fields are nested underneath). A `time`
field (e.g. `timeUnixNano`) is an integer count of nanoseconds since the Unix epoch.
"""


def build_application(
    registry: "ConnectionRegistry", programs: "list[dict[str, Any]]", ip: str
) -> "Starlette":
    """Build the MCP streamable-HTTP application over ``registry`` and ``programs``.

    The returned Starlette app serves the MCP endpoint at the ``/mcp`` path
    (FastMCP's default). The gateway mounts its REST API onto this same app, so
    this is also the app whose lifespan runs the streamable-HTTP session manager.

    ``ip`` is the address the gateway will be *served* on, and it must be, because
    FastMCP derives its DNS-rebinding protection from it: told a loopback address
    it rejects any request whose Host header is not loopback, and told anything
    else it leaves the Host check off. Left to FastMCP's own default the check
    would be armed for loopback no matter where the gateway actually listens, so
    a gateway bound to ``0.0.0.0`` would answer ``421 Misdirected Request`` to
    every client that reached it on a real interface address.
    """
    server = FastMCP("Dusk", instructions=INSTRUCTIONS, host=ip)
    register_tools(server, registry, programs)
    return server.streamable_http_app()


def register_tools(
    server: "FastMCP", registry: "ConnectionRegistry", programs: "list[dict[str, Any]]"
) -> None:
    """Register every tool onto ``server``."""

    async def connect(host: str, port: int, context: Context) -> str:
        # Async so the session-end hook is registered on the event loop that
        # owns the session; the blocking dusk connect runs off it in a thread.
        session = context.session
        descriptor, is_first_connection = await anyio.to_thread.run_sync(
            registry.connect, session, host, port
        )
        if is_first_connection:

            async def disconnect_owner_on_close() -> None:
                # Runs from BaseSession.__aexit__'s exit-stack unwind, which on
                # server shutdown happens while this task is already cancelled.
                # to_thread.run_sync issues a cancellation checkpoint, so without
                # the shield the CancelledError re-raises mid-unwind and corrupts
                # anyio's cancel-scope stack ("Attempted to exit a cancel scope
                # that isn't the current task's current cancel scope").
                with anyio.CancelScope(shield=True):
                    await anyio.to_thread.run_sync(registry.disconnect_owner, session)

            session._exit_stack.push_async_callback(disconnect_owner_on_close)
        return descriptor

    server.add_tool(
        connect,
        name="connect",
        title="Connect to a dusk server",
        description=(
            "Open a connection to a dusk server at the given host and port. "
            "Returns a descriptor string — eight hexadecimal digits — that "
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
        return await _drain(connection.sh(command))

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


async def _drain(output) -> str:
    """Collect everything a command produced into a single text result.

    Each value is rendered as JSON where possible, falling back to ``repr`` for
    objects JSON cannot serialize. One value per line. Awaited rather than
    iterated, so a tool call that waits an hour on a quiet node holds a task
    instead of one of the process's worker threads.
    """
    rendered = []
    while True:
        try:
            item = await output.next_value()
        except StopAsyncIteration:
            return "\n".join(rendered)
        try:
            rendered.append(json.dumps(item, default=str))
        except TypeError, ValueError:
            rendered.append(repr(item))
