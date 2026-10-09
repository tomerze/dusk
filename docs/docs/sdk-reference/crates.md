# Crates

Dusk is a Cargo workspace. Its crates split along the same layers as the
[architecture](../development/architecture.md): the portable core, the programs,
the impls, and the clients.

## Core (`dusk/src/`)

| Crate | Role |
|-------|------|
| `dusk_capnp` | The Cap'n Proto schemas (`dusk.capnp`, `stream.capnp`) - the [wire format](capnp-schemas.md). Depend on it to talk to a node from outside the tree. |
| `dusk_program` | The SDK a program implements: the `ProcessMixin` / `LauncherMixin` traits, `Namespace`, `ProgramArgs`, `Signal`, `Ready`, and the stream helpers. `no_std`. |
| `dusk_program_proc` | The [proc macros](proc-macros.md) that remove the boilerplate - `metadata!`, `derive(Args)`, `derive(Launcher)`, `derive(Process)`, `derive(Portal)`, and the `impl_*_rpc_server` attributes. |
| `dusk_program_sh_proc` | The `#[sh_entry]` attribute macro. |
| `dusk_core` | The runtime: the `DuskServer` behind the `Dusk` capability, the `Driver` trait and its extern shim, sessions, and `init` wiring. `no_std`. |
| `dusk_build` | Build-script helpers for compiling `.capnp` schemas. |
| `dusk_llm` | The client behind [Ask Dusk](../features/ask-dusk.md): builds the prompt and calls a configured OpenAI-compatible endpoint. |

## Programs (`base/`)

Each Base program is its own crate, named `dusk_program_<name>` - `dusk_program_sh`
(the shell, which also hosts the `ShEntry` / `SH_ENTRIES` registry), `dusk_program_ps`,
`dusk_program_kill`, `dusk_program_sleep`, `dusk_program_date`,
`dusk_program_hostname`, `dusk_program_true`, `dusk_program_false`,
`dusk_program_init`, `dusk_program_nightfall`, `dusk_program_logs`. See
[Base programs](../getting-started/concepts/base.md). `dusk_program_sh_compiler`
(`base/sh/compiler/`) sits under the shell's client side: it holds the grammar a
script is parsed with, and a node never links it. The bytecode a script compiles
into, which `init` and `sh` both take in their args, is `sh`'s own `bytecode.capnp`.
See [The shell](../development/shell.md).

`dusk_base` ties them together: it re-exports the programs and provides
`default_launcher_set()`, the set a node links.

## Clients (`dusk/src/`)

| Crate | Role |
|-------|------|
| `dusk_program_sh` (`client::prompt`, `client::shell`) | The interactive prompt - reedline UI, builtins, output rendering, host of [Ask Dusk](../features/ask-dusk.md) - and the `Shell` that drives the `sh` process a client attaches to. Both are the `sh` program's client side. |
| `dusk_cli` | The `dusk` [CLI](cli.md). |
| `dusk_py` | The [Python](python-api.md) extension (PyO3, built with maturin). |
| `dusk_connection` | `Connection` - a client's link to a node, over plain TCP or over TLS. See [below](#dusk_connection). |

### `dusk_connection`

`Connection::connect(address)` connects to a node at a `SocketAddr` over plain
TCP. `Connection::connect_tls(host, port, tls)` connects over TLS 1.3 or 1.2, to
a server that terminates TLS in front of a node; `host` is a host name or an IP
address, and each address it resolves to is tried in turn. Either way
`connection.client()` hands out the node's `Dusk` capability:

```rust
use dusk_connection::{Connection, TlsClient};

let connection = Connection::connect_tls(
    "node.example.internal",
    8444,
    TlsClient {
        server_name: "node.example.internal".to_string(),
        ca: "/etc/client/ca.pem".into(),
        certificate: Some("/etc/client/client.pem".into()),
        key: Some("/etc/client/client.key".into()),
    },
)
.await?;
let dusk = connection.client().await;
```

`server_name` is the name the server's certificate must carry, and `ca` a PEM
file of the certificates it must chain to; `certificate` and `key`, PEM files of
this client's certificate chain and its private key, go together. `connect_tls`
fails at once when a file can't be read. The files are read again on every
connect and reconnect, so a certificate renewed on disk is used without a new
connection.

The link is made on the first call, not by `connect` or `connect_tls`. A link
that can't be made - a closed port, a name that does not resolve, a refused
handshake, no answer within 10 seconds - or that breaks fails the call with
`Disconnected`, and the next call makes it again, after a random wait that
grows with each failure in a row, up to 30 seconds. Once
`connection.disconnect()` has run, a `Dusk` kept from the connection never
connects again: its calls fail.

## Impls (`impls/`)

| Crate | Role |
|-------|------|
| `dusk_nix` | The Unix impl: hosts the Embassy executor, implements `NixDriver`, enables `embassy-time/std`, and accepts client connections. Reads the hostname with `gethostname(2)`. |
| `dusk_windows` | The Windows impl: the same, implementing `WindowsDriver`, and reading the hostname with `GetComputerNameW`. |
| `dusk_std` | The std impl: hosts the Embassy executor, implements `StdDriver`, and recovers a node's exit code the same way nix and windows do. It names no platform, so it compiles for every target with std and threads - ESP-IDF, Windows, Android, iOS, macOS, the BSDs, illumos and Linux. Its `hosted` feature, on by default, supplies `critical-section/std` and Embassy's std platform; firmware that brings its own turns it off. |

## Artifacts (`artifacts/`)

The deliverables you ship - templates to copy and make your own.

| Crate | Role |
|-------|------|
| `dusk_node` | The packaged server: a Rust rlib, a C library, or a staticlib to link into firmware or any host application. Which impl it links is a cargo feature - `impl_nix`, `impl_windows` or `impl_std`. See [Node artifacts](../embedding/node-artifacts.md). |
| `dusk_node_bin` | Wraps it as the `dusk_node` binary, taking the same feature. |
| `dusk_cli_bin` | Builds the `dusk` CLI binary. |
| `dusk_py` (artifact) | Builds the `dusk` Python extension. |
