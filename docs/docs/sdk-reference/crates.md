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
`dusk_program_init`, `dusk_program_logs`. See
[Base programs](../getting-started/concepts/base.md).

`dusk_base` ties them together: it re-exports the programs and provides
`default_launcher_set()`, the set a node links.

## Clients (`dusk/src/`)

| Crate | Role |
|-------|------|
| `dusk_program_sh` (`client::prompt`, `client::shell`) | The interactive prompt - reedline UI, builtins, output rendering, host of [Ask Dusk](../features/ask-dusk.md) - and the `Shell` that drives the `sh` process a client attaches to. Both are the `sh` program's client side. |
| `dusk_cli` | The `dusk` [CLI](cli.md). |
| `dusk_py` | The [Python](python-api.md) extension (PyO3, built with maturin). |

## Impls (`impls/`)

| Crate | Role |
|-------|------|
| `dusk_nix` | The Linux impl: hosts the Embassy executor, implements `NixDriver`, enables `embassy-time/std`, and accepts client connections. |

## Artifacts (`artifacts/`)

The deliverables you ship - templates to copy and make your own.

| Crate | Role |
|-------|------|
| `dusk_node` / `dusk_node_bin` | The packaged server: a Rust rlib, a C library, or the `dusk_node` binary. See [Embedding](../embedding/index.md). |
| `dusk_cli_bin` | Builds the `dusk` CLI binary. |
| `dusk_py` (artifact) | Builds the `dusk` Python extension. |
