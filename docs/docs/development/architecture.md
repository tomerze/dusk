# Architecture

How a Dusk node is put together, and the seams that make one platform span very
different hardware.

## Crate layout

A node is assembled from the core crates (`dusk_core`, `dusk_capnp`,
`dusk_program`, …), a set of programs, and one impl. The
[SDK Reference › Crates](../sdk-reference/crates.md) page lists each crate and its
role; this page is about how they interact at runtime.

## Client / server split

The single most load-bearing rule in the codebase: **the server is `no_std`, the
client is `std`.** A program crate contains both sides, separated by a `client`
Cargo feature:

- **Server side** — the unconditional code, compiled into the node. It must be
  `no_std`-clean: `alloc` instead of `std`, no std-only dependencies.
- **Client side** — everything behind `#[cfg(feature = "client")]`, compiled into
  the CLI. `std` is fine here.

The same rule extends outward: `dusk_core` and `dusk_program` are `no_std`; impls
(`dusk_nix`) and the client crates (`dusk_cli`, `dusk_py`) are `std`, as is the
`client` half of every program crate — the prompt under `base/sh/src/client/`
included.

## A client session

A node has exactly one [namespace](../getting-started/concepts/namespaces.md),
created at startup. The node's `init` process binds the TCP listener and, for
each incoming connection, spawns a `dusk_core` **session** task that shares that
one namespace — so all connected clients see the same processes. The session
wraps a `DuskServer` as a Cap'n Proto bootstrap capability and runs an RPC system
over the stream. The client now holds a `Dusk` capability and can call into the
node.

## `Dusk.process` → `Dusk.run`

Starting work is two steps. `Dusk.process(programArgs)` gets the node's
[`LauncherSet`](../getting-started/concepts/launchers.md) from the
[driver](../getting-started/concepts/drivers-and-impls.md)'s `launchers(namespace)`
hook and launches a [process](../getting-started/concepts/processes.md) — the
launcher is chosen by a **local** read of the args' program id (no network call).
Then either `Dusk.run(process)` spawns it as its own task (a daemon that outlives
the session) or `process.run()` runs it inside the calling session.

## Link-time dispatch

Two seams are resolved by the linker rather than by data:

- **The driver shim.** `dusk_core` calls `unsafe extern "Rust"` symbols
  (`_dusk_hostname`, `_dusk_exit`, `_dusk_launchers`) that the impl defines via
  `dusk_driver_impl!`. `dusk_core` depends on no impl; the impl satisfies the
  symbols. See [Drivers & Impls](../getting-started/concepts/drivers-and-impls.md).
- **The shell entry table.** Shell-invocable programs register into a
  `#[distributed_slice]` `SH_ENTRIES` table at link time — in the client binary,
  where shell entry resolution happens — so the shell can resolve a shell entry
  name to a program without any registry being built at runtime.

Both seams are what let the pieces be mixed and matched — programs and impls
into a node, shell-invocable programs into a client — without any of them
depending on each other directly.
