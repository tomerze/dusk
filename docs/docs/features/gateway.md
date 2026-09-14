# API gateway

Dusk ships an **API gateway** - `dusk_gw` - that puts a dusk node's [shell](shell.md) programs behind HTTP. It serves two surfaces from one process:

- a **REST API** under `/v1`, one endpoint per method of the [Python `Dusk` class](../sdk-reference/python-api.md), for scripts, dashboards, and anything that speaks HTTP - self-describing, with an [OpenAPI document and a Swagger UI](#openapi-and-swagger-ui);
- an **[MCP](https://modelcontextprotocol.io) server** at `/mcp`, one tool per dusk program, for an LLM agent or an IDE.

Both drive [Base](../getting-started/concepts/base.md) programs on nodes - much like the interactive `dusk` CLI, but over the network and without a terminal.

It lives in the `dusk` Python package (built from `dusk_py`), under `dusk.gw`.

## Gateway model

**The gateway is what holds the connections.** You do not connect to a node and then tell the gateway about it - you ask the gateway to open one, and it does, keeping the live connection in its own process for as long as it is open. What comes back is a *descriptor* naming that connection; every later call names the descriptor, and the gateway does the talking to the node on your behalf.

This is what makes it a gateway rather than a proxy: it is not forwarding your connection, it is holding its own. The consequences are worth knowing up front - connections outlive the HTTP request that opened them, they die when the gateway process does, and a gateway with connections open is holding fleet state that nothing else has a copy of.

A descriptor is a connection handle: eight hexadecimal digits, such as `a3f91c07`. You pass it on every later call to say *which* connection to act on, and to `disconnect` when you are done. It names a connection, not a node - two connections to the same node get two descriptors - and it carries no trace of the node it reaches, so it cannot be read back to find out where a connection points.

You can hold several connections - to different nodes or the same node - at once, each identified by its own descriptor.

**Descriptors belong to whoever opened them.** Every connection opened over REST belongs to one shared REST owner, so any REST caller may use any REST descriptor. Every connection an MCP session opens belongs to that session alone. Neither surface can reach the other's connections, even though they share one registry and one descriptor format.

## REST API

Every endpoint lives under `/v1`, takes and returns JSON, and mirrors one method of the Python `Dusk` class:

| Endpoint | Python equivalent |
|----------|-------------------|
| `POST /v1/connect` | `Dusk(host, port)` |
| `POST /v1/disconnect` | `Dusk.disconnect()` |
| `POST /v1/sh` | `Dusk.sh(command)` |
| `POST /v1/sh/stream` | `Dusk.sh(command)`, streamed value by value |
| `GET /v1/help` | `Dusk.help()` |
| `GET /v1/help/{program}` | `Dusk.help(program)` |

Open a connection and keep the descriptor:

```bash
curl -s localhost:9100/v1/connect \
     -H 'content-type: application/json' \
     -d '{"host": "127.0.0.1", "port": 9090}'
```

```json
{"descriptor": "a3f91c07"}
```

Run a command on it. `command` is a [Dusk shell](shell.md) command line, exactly what you would type at the `dusk` prompt:

```bash
curl -s localhost:9100/v1/sh \
     -H 'content-type: application/json' \
     -d '{"descriptor": "a3f91c07", "command": "ps"}'
```

```json
{"output": [{"0xcef2c7c974bf44ec": {"pid": 1, "program": "init"}}]}
```

`output` is a list holding one entry per value the program produced, in order. A program that produces nothing gives `{"output": []}`.

Close it when finished:

```bash
curl -s localhost:9100/v1/disconnect \
     -H 'content-type: application/json' \
     -d '{"descriptor": "a3f91c07"}'
```

`GET /v1/help` needs no connection: the program set is baked into the gateway at link time, so it can be read before any node is reached.

```bash
curl -s localhost:9100/v1/help
curl -s localhost:9100/v1/help/logs
```

### OpenAPI and Swagger UI

The API describes itself. `GET /v1/openapi.json` is an OpenAPI 3.1 document, and `GET /v1/docs` is a **Swagger UI** over it - open it in a browser to read the endpoints and call them against a live node.

```bash
curl -s localhost:9100/v1/openapi.json     # the document
open http://localhost:9100/v1/docs         # the browsable interface
```

The document is generated from the same pydantic models that validate incoming requests, so it cannot drift from what the gateway actually accepts. It covers `/v1` only - MCP negotiates its own capabilities in the protocol handshake and is not described here.

Point any OpenAPI client generator at `/v1/openapi.json` to get a typed client in your language.

**The docs page needs no internet.** Swagger UI's JavaScript and CSS are vendored into the `dusk` package and served by the gateway itself from `/v1/static/`, and the page requests nothing from anywhere else - no CDN, no web font, not even a favicon. It renders identically on a host with no route off its own network, which is where a fleet usually sits. The gateway's tests assert this by parsing the page and failing on any external URL, so it cannot regress quietly.

The vendored copy is `swagger-ui-dist` 5.32.14; `dusk/src/dusk_py/python/dusk/gw/static/README.md` records the file hashes and how to update them.

### Errors

Failures are JSON too - `{"error": "..."}` with the status code:

| Status | When |
|--------|------|
| `400` | The body is not a JSON object, a required field is missing or of the wrong type, or it carries a field the endpoint does not take. |
| `404` | The descriptor is unknown (never minted, or already disconnected), or no such program. |
| `502` | The node refused the connection or could not be reached. |

Request bodies are read **strictly**: no type is coerced into another. `{"port": "9090"}` is a `400`, not port 9090, and `{"port": true}` is a `400`, not port 1 - a gateway that guesses would connect somewhere you did not ask for. Unrecognised fields are rejected rather than ignored, so a typo in a field name fails loudly.

There is no `422`. A body the gateway cannot read is a `400` like every other malformed request, and the OpenAPI document says so.

### Streaming output

`POST /v1/sh` returns when the program finishes, so a program that never finishes on its own - a live `logs` follow - would hold the request open forever.

`POST /v1/sh/stream` takes the same body and sends each value **as the program produces it**, as a [Server-Sent Events](https://developer.mozilla.org/docs/Web/API/Server-sent_events) stream:

```bash
curl -N localhost:9100/v1/sh/stream \
     -H 'content-type: application/json' \
     -d '{"descriptor": "a3f91c07", "command": "logs dump"}'
```

```
event: start
data: {}

event: output
data: {"0x9f3a…": {"body": "node started", "severity": "info"}}

event: output
data: {"0x9f3a…": {"body": "listening on 0.0.0.0:9090", "severity": "info"}}
```

(`curl -N` disables curl's own buffering. Without it curl will hold the output.)

Four kinds of event are sent:

| Event | Meaning |
|-------|---------|
| `start` | The stream is live. Sent immediately, before the program has produced anything, so a quiet command is distinguishable from a gateway that never answered. |
| `output` | One value the program produced. `data` is that value as JSON - the same shape `/v1/sh` puts in its `output` list. |
| `end` | The program finished. Nothing follows. |
| `error` | The command failed partway through. `data` is `{"error": "..."}`. Nothing follows. |

A failure that happens *before* the stream starts - an unknown descriptor, a malformed body - is a normal `404` or `400` with a JSON body, because no response has begun yet. Once the first event is out the status code is already sent, which is why a later failure has to arrive as an `error` event instead.

**Disconnecting stops you reading; it does not stop the program.** The command keeps running on the node until it finishes or you `kill` it, exactly as with the blocking endpoint.

**An open stream costs a task, not a thread.** The gateway awaits each value rather than blocking on it, so a command that stays quiet for an hour occupies nothing but memory. Measured on a live node: 3,753 values drained on a single thread, and twelve simultaneous commands still on that one thread.

**The limit you will actually hit is the node's.** A node runs each command in a shell task from a pool of 16 (`pool_size` in `base/sh/src/lib.rs`), so past roughly a dozen simultaneous commands *against one node* it answers `Busy - Too many instances of this task are already running`, which the gateway passes back to you: a `502` from `/v1/sh`, and an `error` event from `/v1/sh/stream`, since by then the response has already begun. This is a per-node limit, so more nodes means more concurrency; more gateways does not.

Reading the stream from Python:

```python
import httpx

with httpx.stream("POST", "http://localhost:9100/v1/sh/stream",
                  json={"descriptor": "a3f91c07", "command": "logs dump"}) as response:
    for line in response.iter_lines():
        if line.startswith("data: "):
            print(line.removeprefix("data: "))
```

Browsers have `EventSource`, but it only issues `GET` requests with no body, so it cannot call this endpoint; use `fetch` with a `ReadableStream` reader instead.

## MCP server

MCP is served at `/mcp` over **streamable HTTP**, so MCP clients connect to `http://<host>:<port>/mcp`. **It is always on** - the gateway mounts it unconditionally and there is no switch to turn it off.

The server registers:

- **`connect(host, port)`** - open a connection to a dusk node. Returns a new descriptor.
- **`disconnect(descriptor)`** - close a connection. The descriptor is invalid afterward.
- **one tool per dusk program** - named after the program (`ps`, `kill`, `sleep`, …). Each takes a `descriptor` plus a freeform `arguments` string, and runs `<program> <arguments>` over that connection's shell, returning the output.

The program tools are enumerated from the same link-time program set `/v1/help` reports, so they are known without any connection. The tool descriptions carry each program's short and long help text; an agent reads those to fill in `arguments`.

### Long-running programs

Every program tool supports MCP **task-augmented invocation** (`execution.taskSupport: "optional"`). A client that invokes a program tool as a task gets a task id back immediately while the program runs in the background on the gateway; it polls the task and fetches the result when the program finishes - the model keeps working in the meantime. A plain (non-task) call returns when the program completes, so clients without task support are unaffected.

Two things to know:

- Cancelling a task does not kill the program on the node - the program keeps running; use the `kill` tool for that.
- Task results are held in gateway memory until fetched, so a pending result does not survive a gateway restart.

## Running it

Nothing is served from the bare root: `/` returns 404. Clients reach `/v1/…` or `/mcp`.

### Command line

```bash
dusk_gw <ip> <port>
```

or, equivalently, without the installed console script:

```bash
python -m dusk.gw <ip> <port>
```

Binds the gateway to `ip:port` and blocks. For example, `dusk_gw 0.0.0.0 9100` serves it on all interfaces at port 9100.

### From Python

```python
import dusk.gw

dusk.gw.serve("0.0.0.0", 9100)   # blocks, runs uvicorn for you
```

`serve` is the batteries-included wrapper: it runs uvicorn with defaults and nothing else. `dusk_gw` on the command line is exactly this.

### Serving the app yourself

`serve` takes no options beyond an address and a port. Anything else - TLS, logging, timeouts, workers, a different ASGI server, extra middleware, mounting the gateway inside a larger application - you get by taking the **ASGI application** from `dusk.gw.app()` and serving it however you like. `serve` is a two-line convenience; this is the real interface.

```python
import uvicorn
import dusk.gw

application = dusk.gw.app("0.0.0.0")

uvicorn.run(
    application,
    host="0.0.0.0",
    port=9100,
    ssl_keyfile="key.pem",          # TLS - see HTTPS below
    ssl_certfile="cert.pem",
    log_config=my_logging_config,   # your logging, not uvicorn's defaults
    timeout_keep_alive=75,
    proxy_headers=True,             # behind a reverse proxy
    forwarded_allow_ips="10.0.0.0/8",
)
```

It is a standard ASGI app, so it is not tied to uvicorn. `app()` takes an argument, so give your server a module with the application already built rather than an `import:attribute` path to `app` itself:

```python
# mygateway.py
import dusk.gw

application = dusk.gw.app("0.0.0.0")
```

```bash
hypercorn mygateway:application --bind 0.0.0.0:9100
granian --interface asgi --host 0.0.0.0 --port 9100 mygateway:application
```

### Mounting it inside another application

You can serve the gateway under a prefix of an application you already run. **Forward its lifespan**, or the MCP endpoint will fail every request with `Task group is not initialized` - Starlette does not run a mounted app's lifespan, and that is where MCP's session manager is started:

```python
import contextlib

from starlette.applications import Starlette
from starlette.routing import Mount
import dusk.gw

gateway = dusk.gw.app("0.0.0.0")


@contextlib.asynccontextmanager
async def lifespan(site):
    async with gateway.router.lifespan_context(gateway):
        yield


site = Starlette(
    routes=[
        Mount("/dusk", gateway),
        # ... your own routes
    ],
    lifespan=lifespan,
)
```

The REST API is then at `/dusk/v1/…` and MCP at `/dusk/mcp`. The OpenAPI document, the Swagger UI and its assets follow the mount by themselves - the document's `servers` becomes `/dusk/v1`, so a generated client targets the right prefix without being told.

Forwarding the lifespan is also what closes the gateway's connections when your application shuts down.

**`app()`'s parameters:**

| Parameter | What it is |
|-----------|-----------|
| `ip` (first, positional) | The address you will serve on. It binds nothing - that is the server's job - but the app has to be told; see [Bind address and MCP](#bind-address-and-mcp). Defaults to `127.0.0.1`. |
| `connection_factory` | Replaces the `Dusk` class the gateway opens connections with. For drivers that are not real nodes - a test double, an instrumented client. |
| `programs` | Replaces the program set `/v1/help` reports and the MCP tools are built from. Defaults to what the linked dusk impl provides. |

The last two are what let the gateway's own tests run with no node and no compiled extension.

**Serve a single worker.** The connection registry and the MCP task store are in-process state. With more than one worker process each holds its own, so a descriptor minted by one is unknown to another - see [Single worker only](#single-worker-only).

### HTTPS

`dusk_gw` and `serve` speak **plain HTTP**. Neither takes a certificate, so anything
you serve with them is unencrypted, including the node addresses and command output
that pass through.

To serve HTTPS, build the app yourself and give the certificate to the ASGI server:

```python
import uvicorn
import dusk.gw

uvicorn.run(dusk.gw.app("0.0.0.0"), host="0.0.0.0", port=9100,
            ssl_keyfile="key.pem", ssl_certfile="cert.pem")
```

Terminating TLS at a reverse proxy in front of the gateway works as well, and is
the usual choice where one is already in place.

Either way, the gateway has **no authentication of its own**: anyone who can reach
it can open connections to any node it can reach, and use any descriptor another
REST caller minted. Put it somewhere only the people who should drive your fleet
can reach, and if it needs to be exposed, put something in front of it that
authenticates.

### Bind address and MCP

The MCP endpoint carries DNS-rebinding protection, and it is armed from the address the gateway is served on:

- served on a **loopback** address (`127.0.0.1`, `localhost`, `::1`), it accepts only requests whose `Host` header is a loopback address - the protection is on;
- served on **anything else** (`0.0.0.0`, a specific interface), the `Host` check is off, since binding a routable address is an explicit decision to serve the network.

`dusk_gw <ip> <port>` and `serve(ip, port)` pass the address they bind, so this is handled for you. If you build the app yourself, **pass the same address you will serve on**: an app built with the default `127.0.0.1` but served on `0.0.0.0` answers `421 Misdirected Request` to every MCP client that reaches it on a real interface address. The REST API under `/v1` does not check `Host` and is unaffected either way.

## Single worker only

The connection registry is **in-process state**. Serve the gateway with a single worker: multiple worker processes would each hold a separate, unshared registry, so a descriptor minted by one worker would be unknown to another. The MCP task store is in-process too - a task started on one worker could not be polled on another. `serve` runs a single worker; if you run the app yourself, do the same.

## Connection lifetime

Connections are closed for you:

- when an MCP session ends, every connection that session opened is closed;
- when the gateway shuts down, every connection still open is closed.

A REST descriptor's owner is the gateway process itself, so a REST connection nobody disconnects lives until the gateway stops. Call `disconnect` when you are done with one.

## No interactive views

The gateway sets `DUSK_NON_INTERACTIVE=1` in its process. Interactive
programs - `logs view`, and `sh --prompt` - refuse to run under it,
since they would take over a terminal the caller doesn't have and hang the
request forever. To read logs over the gateway, use `logs dump` (add
`--replay-only` for a bounded snapshot), which returns the entries as the
result; to run a shell command, pass it: `sh <command>`.
