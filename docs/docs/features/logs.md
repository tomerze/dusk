# Logs

Every Dusk node keeps a rolling buffer of the structured log records its programs
emit — and the [tracing spans](../development/tracing.md) behind them. The `logs`
command reads that buffer: live in an interactive viewer, or streamed out to a
file or a collector. It's a [Base program](../getting-started/concepts/base.md),
reached from the [shell](shell.md).

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

You can setup an OpenTelemetry collector to stream otlp:// logs to Elasticsearch.

When configurating the collector it isrecommended to set [Elasticsearch
exporter](https://github.com/open-telemetry/opentelemetry-collector-contrib/tree/main/exporter/elasticsearchexporter)'s
mapping mode to **`ecs`**.

Dusk stamps each record with `elasticsearch.document_id`, so replays dedupe
seamlessly — turn on the `logs_dynamic_id` flag in your otel collector's elasticsearch exporter config and the exporter uses it as the document
`_id` (a re-streamed record overwrites its earlier copy instead of duplicating). 

```yaml
# otel-collector.yaml — verified on OpenTelemetry Collector Contrib v0.142.0 (released 15 December 2025)
exporters:
  elasticsearch:
    endpoint: http://elasticsearch:9200
    mapping:
      mode: ecs            # not the default `otel`
    logs_dynamic_id:
      enabled: true        # use dusk's elasticsearch.document_id as the doc _id, so replays dedupe
```

## Modes

By default `logs` replays the buffered history and then follows new logs forever.
Two flags change that:

- `--replay-only` — replay the history, then stop: a bounded snapshot that
  returns, for scripts and MCP (a plain stream never returns). With no url it
  prints the buffer to stdout.
- `--follow-only` — skip the history; follow only logs from now on.

## Filter by level

```sh
logs -l warn                  # only warn and above
logs stream file://out.jsonl -l error
```

`-l` / `--level` sets the minimum severity to show or stream: `error`, `warn`,
`info`, `debug`, or `trace` (the default — everything).

## Spans

Dusk records [tracing spans](../development/tracing.md) alongside log records, and
where they surface depends on the destination:

- in the **viewer**, spans aren't shown — logs only;
- streamed to **`file://`** or **`http(s)://`**, they appear as ordinary
  structured logs (without a severity);
- streamed to **`otlp://`**, they appear as proper OTLP traces.
