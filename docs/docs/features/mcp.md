# MCP

Dusk ships an [MCP](https://modelcontextprotocol.io) server that exposes a dusk node's [shell](shell.md) programs as MCP tools. It lets an MCP client (an LLM agent, an IDE, etc.) connect to dusk nodes and run [Base](../getting-started/concepts/base.md) programs on them — much like the interactive `dusk` CLI, but driven by a model instead of a person.

It lives in the `dusk` Python package (built from `dusk_py`), under `dusk.mcp`.

## Gateway model

The MCP server holds **no dusk connection of its own**. It is a *gateway*: an MCP client opens connections to nodes through it, and the server hands back a descriptor for each one.

A descriptor is a human-readable connection handle formatted `host:port#n`, where `n` is a per-server counter that keeps repeat connections to the same address distinct. You pass the descriptor to every program tool to say *which* node to run on, and to `disconnect` when you are done.

You can hold several connections — to different nodes or the same node — at once, each identified by its own descriptor.

## Tools

The server registers:

- **`connect(host, port)`** — open a connection to a dusk node. Returns a new descriptor.
- **`disconnect(descriptor)`** — close a connection. The descriptor is invalid afterward.
- **one tool per dusk program** — named after the program (`ps`, `kill`, `sleep`, …). Each takes a `descriptor` plus a freeform `arguments` string, and runs `<program> <arguments>` over that connection's shell, returning the output.

The program tools are enumerated from `Dusk.help()` — the link-time program set baked into the dusk impl — so they are known without any connection. The tool descriptions carry each program's short and long help text; an agent reads those to fill in `arguments`.

## Running it

The MCP server speaks **streamable HTTP**. The endpoint is served at the `/mcp` path (FastMCP's default), so clients connect to `http://<host>:<port>/mcp` — the bare `host:port` returns 404.

### Command line

```bash
python -m dusk.mcp <ip> <port>
```

Binds the gateway to `ip:port` and blocks. For example, `python -m dusk.mcp 0.0.0.0 9100` serves it on all interfaces at port 9100.

### From Python

```python
import dusk.mcp

dusk.mcp.serve("0.0.0.0", 9100)   # blocks, runs uvicorn for you
```

`serve` is the batteries-included wrapper. For full control over the run parameters (TLS, logging, etc.), build the ASGI app yourself and hand it to your own server:

```python
import uvicorn
import dusk.mcp

uvicorn.run(dusk.mcp.app(), host="0.0.0.0", port=9100,
            ssl_keyfile=..., log_config=...)
```

## Workflow

From the client's side, a session looks like:

1. Call **`connect`** with a node's host and port. Keep the returned descriptor.
2. Call the **per-program tools** (`ps`, `kill`, …) with that descriptor and any arguments to run commands and read their output.
3. Call **`disconnect`** with the descriptor when finished.

## Long-running programs

Every program tool supports MCP **task-augmented invocation** (`execution.taskSupport: "optional"`). A client that invokes a program tool as a task gets a task id back immediately while the program runs in the background on the gateway; it polls the task and fetches the result when the program finishes — the model keeps working in the meantime. A plain (non-task) call returns when the program completes, as before, so clients without task support are unaffected.

Two things to know:

- Cancelling a task does not kill the program on the node — the program keeps running; use the `kill` tool for that.
- Task results are held in gateway memory until fetched, so a pending result does not survive a gateway restart.

## Single worker only

The connection registry is **in-process state**. Serve the gateway with a single worker: multiple worker processes would each hold a separate, unshared registry, so a descriptor minted by one worker would be unknown to another. The task store is in-process too — a task started on one worker could not be polled on another. `serve` runs a single worker; if you run the app yourself, do the same.

## No interactive views

The gateway sets `DUSK_NON_INTERACTIVE=1` in its process. Interactive
programs — `logs view` — refuse to run under it, since they would take over
a terminal the model driving the gateway doesn't have and hang the tool call
forever.
