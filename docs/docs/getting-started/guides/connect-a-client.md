# Connect a client

Connecting to a node is how you get Dusk's [analytics and
diagnosis](../index.md#what-you-get) — see what's running, read logs, and drive a
device. Anything that holds the node's `Dusk` capability can do it; there are
three first-class ways in.

## The `dusk` CLI

The interactive prompt, backed by `dusk_prompt`. Point it at a node's address and
you get a shell connected to that node:

```bash
cargo run --bin dusk -- 127.0.0.1:9090
```

Pass a command as a second argument to run it once and exit instead of opening
the prompt:

```bash
cargo run --bin dusk -- 127.0.0.1:9090 "ps"
```

This is the human entry point — type commands, read output, define functions.
See the [Shell](../../features/shell.md) reference for the command language.

## The Python API

`dusk_py` exposes a node to Python. Build the extension, then connect and run
programs:

```bash
uv run maturin develop
uv run python
```

```python
import dusk

node = dusk.Dusk('127.0.0.1', 9090)
# run `ps` on the node and print the processes it returns
print(list(node.sh('ps')))
node.disconnect()
```

`node.sh(command)` returns an iterator over the command's output values. The
static `dusk.Dusk.help()` lists the available programs without a connection. This
is the programmatic entry point — scripting fleet operations, integrating with
existing tooling.

## The MCP gateway

For LLM agents and IDEs, Dusk ships an MCP server that exposes a node's programs
as MCP tools. An agent connects to nodes through the gateway and runs commands on
them much like the CLI, but driven by a model. See
[Features › MCP](../../features/mcp.md) for the gateway model and how to run it.
