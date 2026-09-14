# Connect a client

Connecting to a node is how you get Dusk's [analytics and
diagnosis](../index.md#what-you-get) - see what's running, read logs, and drive a
device. Anything that holds the node's `Dusk` capability can do it; there are
three first-class ways in.

## The `dusk` CLI

The interactive prompt, the client side of the `sh` program. Point it at a node's address and
you get a shell connected to that node:

```bash
cargo run --bin dusk -- 127.0.0.1:9090
```

Pass a command as a second argument to run it once and exit instead of opening
the prompt:

```bash
cargo run --bin dusk -- 127.0.0.1:9090 "ps"
```

This is the human entry point - type commands, read output, define functions.
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
is the programmatic entry point - scripting fleet operations, integrating with
existing tooling.

## The HTTP API gateway

For everything that isn't Rust or Python - a dashboard, a CI job, another
service, an AI agent - Dusk ships `dusk_gw`, an API gateway that puts a node's
programs behind HTTP. It comes with the same `dusk` package:

```bash
uv run maturin develop          # builds the dusk package, which ships dusk_gw
uv run dusk_gw 0.0.0.0 9100
```

The gateway is what holds the connection. Ask it to open one to a node, and keep
the descriptor it hands back:

```bash
curl -s localhost:9100/v1/connect \
     -H 'content-type: application/json' \
     -d '{"host": "127.0.0.1", "port": 9090}'
```

```json
{"descriptor": "a3f91c07"}
```

Then run commands on that node by naming the descriptor:

```bash
curl -s localhost:9100/v1/sh \
     -H 'content-type: application/json' \
     -d '{"descriptor": "a3f91c07", "command": "ps"}'
```

The API describes itself: open **`http://localhost:9100/v1/docs`** in a browser
for a Swagger UI you can call every endpoint from, and point a client generator
at `/v1/openapi.json` to get a typed client in your language. Neither needs
internet access.

An **MCP** server for LLM agents and IDEs is always served on the same port at
`/mcp`, exposing each of the node's programs as a tool.

See [Features › API gateway](../../features/gateway.md) for the full endpoint
reference, the connection model, and how to serve it over HTTPS.
