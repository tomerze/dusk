# Contributing

How to work in the Dusk codebase.

## Setup

Get the toolchain, build dependencies, and pre-commit hooks in place first — see
[Setup](index.md) and [Installation](../getting-started/guides/installation.md).

## Conventions

A few rules are enforced more strictly than usual:

- **No abbreviations.** Identifiers are spelled out — `request`, not `req`;
  `address`, not `addr`. This holds even for short-lived locals.
- **Respect the `no_std` / `std` split.** Server-side code stays `no_std`-clean;
  std-only code belongs behind the `client` feature. A quick check:
  `grep -rn "std::\|use std" programs/` should only turn up hits in `client.rs`
  files or `#[cfg(feature = "client")]` modules. See
  [Architecture](architecture.md#client-server-split).
- **Trace every task.** Each Embassy task opens a `tracing` span whose **first**
  field is `task_id`, followed by the domain fields (`namespace_id`, `pid`,
  `program_id`, `program_name`). Lifecycle events, dropped errors, and boundary
  calls are logged so a node is diagnosable after the fact.

## Pre-commit and CI

The pre-commit hooks (installed with `uv run pre-commit install`) run formatting
and lint checks on every commit. Run them before pushing so CI stays green.

## Tests

The suite runs under [nextest](https://nexte.st/):

```bash
cargo nextest run
```

Integration tests live under `tests/`.
