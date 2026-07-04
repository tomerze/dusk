# Logs

Every Dusk node keeps a rolling buffer of the structured log records its programs
emit — and the [tracing spans](../development/tracing.md) behind them. The `logs`
command reads that buffer three ways: live in an interactive viewer (`logs view`),
returned to the caller as Dusk values (`logs dump`), or streamed out to a file or
a collector (`logs stream`). It's a
[Base program](../getting-started/concepts/base.md), reached from the
[shell](shell.md).

## View logs interactively

```sh
logs            # open the interactive viewer
logs view       # the same thing
```

`logs` (or `logs view`) opens a read-only, vi-style pager. It replays the
buffered history, then tails new logs live, like `tail -f`. Each line shows the
timestamp, the level (colored by severity), the message, and any `key=value`
fields. It's meant for an interactive context — the dusk prompt or the Python
REPL.

### Keys

| Keys | Action |
|------|--------|
| `h` `j` `k` `l`, arrows | move |
| `gg` / `G` | jump to top / bottom |
| `Ctrl-d` / `Ctrl-u` / `Ctrl-f` / `Ctrl-b` | half / full page |
| `:N` | jump to line N |
| `/` `?`, then `n` / `N` | search forward / back; next / previous match |
| `v` / `V` / `Ctrl-V` | select charwise / linewise / block |
| `y` or `Ctrl-Shift-C` | copy the selection (or the current line) to the clipboard |
| `s` or `:w` | save the logs to a file (see below) |
| `q`, `:q`, `Ctrl-C` | quit back to the shell |

The viewer starts in **FOLLOW**, tailing new logs. Scrolling up pauses it in
**NORMAL** — the view freezes and no new logs arrive until you press `f` or `G`
to follow again. The status bar names the current mode, the line position, and —
while a search is active — a `matched/total` counter.

### Save what you're looking at

`s` (or `:w`) writes the logs the viewer has collected to a file:

```
:w                    # a timestamped file, e.g. /tmp/dusk-logs-20260615-143002.log
:w /path/to/file.log  # a file you name
```

The status bar reports where it landed.

### If the node disconnects

The viewer can't tail a node that's gone, so on a dropped connection it saves what
it collected (the same `/tmp` file as `:w`), prints that path, and returns you to
the prompt — your logs are on disk, not lost with the screen.

## Read logs as values

```sh
logs dump                 # the buffered history, then follow live, as values
logs dump --replay-only   # a bounded snapshot of the buffered history, then stop
logs dump --follow-only   # skip the history; follow new logs as values
```

`logs dump` returns the logs to the caller — one record per log entry — on the
command's own output stream, instead of painting a terminal (`view`) or sending
them to an external sink (`stream`). It's the way to read logs from a script, the
Python REPL, and the [MCP gateway](mcp.md): the records come back as the command's
result.

Each record carries `sequence`, `severity`, `timeUnixNano` (nanoseconds since the
Unix epoch), the `message`, and any `attributes`; a span record carries its
`name`, `startTimeUnixNano`, and — once it has closed — `endTimeUnixNano`
instead. In the dusk prompt
they render as a table; in the Python bindings they arrive as the values a `sh`
call yields.

`logs dump` without `--replay-only` follows forever, like a stream — run it as a
background task and stop it when you're done.

## Stream logs to a destination

```sh
logs stream <url>                # stream forever
logs stream <url> --replay-only  # write the buffered history, then exit
```

`logs stream` routes the same logs to a destination instead of the viewer — use
it from scripts and the [MCP gateway](mcp.md). Supported URLs:

| URL | Destination |
|-----|-------------|
| `file://<path>` | appends to a file, one JSON record per line (jsonl) |
| `otlp://<host:port>` | an OpenTelemetry collector, over OTLP/gRPC |
| `http://…` / `https://…` | POSTs each record as an OTLP/JSON document |

If the destination's connection drops, the stream retries every half-second until
it returns, so a brief collector restart doesn't tear the stream down.

### Streaming to Elasticsearch

You can set up an OpenTelemetry collector to stream `otlp://` logs to
Elasticsearch.

When configuring the collector it is recommended to set the [Elasticsearch
exporter](https://github.com/open-telemetry/opentelemetry-collector-contrib/tree/main/exporter/elasticsearchexporter)'s
mapping mode to **`ecs`**.

Dusk stamps every log record and every span with `elasticsearch.document_id`,
so replays dedupe seamlessly — turn on the `logs_dynamic_id` and
`traces_dynamic_id` flags in your otel collector's Elasticsearch exporter
config and the exporter uses it as the document `_id`. Elasticsearch then
refuses a re-streamed copy as a duplicate of the document it already indexed,
instead of indexing it twice.

```yaml
# otel-collector.yaml — verified on OpenTelemetry Collector Contrib v0.146.0 (released 18 February 2026)
exporters:
  elasticsearch:
    endpoint: http://elasticsearch:9200
    mapping:
      mode: ecs            # not the default `otel`
    logs_dynamic_id:
      enabled: true        # use dusk's elasticsearch.document_id as the log document _id
    traces_dynamic_id:
      enabled: true        # the same for spans; first shipped in v0.146.0
```

A span reaches Elasticsearch twice: once when it opens — with no end time —
and once when it closes, with its end time and final attributes. The two are
separate documents, each with its own `elasticsearch.document_id`, so each
dedupes on replay independently. A span document with no end time is a span
that is still open, or one whose node stopped before it closed.

Log records don't repeat their spans' attributes. To find a process's logs in
Kibana, filter spans by what you know — `pid`, `program_name`, … — take the
matching span's `trace.id` (every task is one trace), and filter logs by it.

## Modes

`logs dump` and `logs stream` both replay the buffered history and then follow new
logs forever by default. Two flags change that:

- `--replay-only` — replay the history, then stop: a bounded snapshot that
  returns, for scripts and MCP (a plain follow never returns).
- `--follow-only` — skip the history; follow only logs from now on.

These flags don't apply to `logs view`, the interactive pager, which always
replays then follows live (scroll up to pause it).

## Filter by level

```sh
logs -l warn                  # only warn and above
logs stream file://out.jsonl -l error
```

`-l` / `--level` sets the minimum severity to show or stream: `error`, `warn`,
`info`, `debug`, or `trace` (the default — everything).

## Spans

Dusk records [tracing spans](../development/tracing.md) alongside log records —
each span twice, once when it opens (no end time) and once when it closes —
and where they surface depends on the destination:

- in the **viewer**, spans aren't shown — logs only;
- from **`logs dump`**, they come back as records alongside the log records; a
  span that is still open has no `endTimeUnixNano`;
- streamed to **`file://`** or **`http(s)://`**, they appear as ordinary
  structured logs (without a severity);
- streamed to **`otlp://`**, they appear as proper OTLP traces.
