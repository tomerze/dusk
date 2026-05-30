# CLI

The `dusk` command-line client connects to a node and drives it — interactively
or one command at a time. It's built from `dusk_cli` (packaged as `dusk_cli_bin`).

## Usage

```
dusk <address> [command] [--debug-console]
```

- **`<address>`** *(required)* — the node's `host:port`, e.g. `127.0.0.1:9090`.
- **`[command]`** *(optional)* — a shell command to run. If given, `dusk` runs it
  once and exits; if omitted, it opens the interactive prompt.
- **`--debug-console`** — enable Tokio console debugging.

Run with no arguments and `dusk` prints its help.

## Examples

```bash
dusk 127.0.0.1:9090            # interactive shell
dusk 127.0.0.1:9090 "ps"       # run one command and exit
```

During development, run it through cargo:

```bash
cargo run --bin dusk -- 127.0.0.1:9090
```

The interactive prompt is also where [Ask Dusk](../features/ask-dusk.md) (Ctrl + A)
lives. For the other ways to reach a node — Python and MCP — see
[Connect a client](../getting-started/guides/connect-a-client.md).
