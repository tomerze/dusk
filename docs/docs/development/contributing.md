# Contributing

How to work in the Dusk codebase.

## Setup

Get the toolchain, build dependencies, and pre-commit hooks in place first - see
[Setup](index.md) and [Installation](../getting-started/guides/installation.md).

## Conventions

A few rules are enforced more strictly than usual:

- **No abbreviations.** Identifiers are spelled out - `request`, not `req`;
  `address`, not `addr`. This holds even for short-lived locals.
- **Respect the `no_std` / `std` split.** Server-side code stays `no_std`-clean;
  std-only code belongs behind the `client` feature. A quick check:
  `grep -rn "std::\|use std" base/ --include="*.rs"` should only turn up hits in `client.rs`
  files or `#[cfg(feature = "client")]` modules. See
  [Architecture](architecture.md#client-server-split).
- **Trace every task.** Each Embassy task opens a `tracing` span that declares
  the task id under `__new_task_id__` (the logs subscriber surfaces it as
  `task_id`), with the domain fields (`namespace_id`, `pid`, `program_id`,
  `program_name`) alongside. Lifecycle events, dropped errors, and boundary
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

## License and the contributor agreement

Dusk is licensed under the GNU Affero General Public License, version 3
only. Every contribution is accepted under the project's
[Contributor Assignment Agreement](../legal/cla.md),
signed once per GitHub account by replying to the check on your first pull
request. The process, from fork to merge, is in
[CONTRIBUTING.md](https://github.com/tomerze/dusk/blob/master/CONTRIBUTING.md),
and how the project is run is in
[GOVERNANCE.md](https://github.com/tomerze/dusk/blob/master/GOVERNANCE.md).
