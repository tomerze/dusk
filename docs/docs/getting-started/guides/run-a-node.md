# Run a dusk node

A node is one running Dusk server — one machine in your fleet. This guide covers
running one beyond the quickstart binary.

## The `dusk_node` binary

A node ships as **`dusk_node`** — the server binary under `artifacts/`, run with
`cargo run --bin dusk_node` (its `main` just calls `dusk_node::dusk_node_run()`).
It links the portable core, a set of programs, and one impl — by default the
**nix** impl (`impls/nix/`, a library, not a binary) for Linux. You make a node
your own by linking in your programs and, if needed, swapping the underlying impl.
See [Build a custom impl](custom-impl.md).

## What happens at startup

Bringing a node up follows a fixed sequence:

1. Create the node's [namespace](../concepts/namespaces.md) with a random id and
   the executor's spawner.
2. Register the impl's [`LauncherSet`](../concepts/launchers.md) against that
   namespace — this is the set of programs the node can run.
3. Spawn the first process, `init`, into the namespace via an in-process `Dusk`
   client (`Dusk.process` + `Dusk.run`).

The `init` process then binds the node's network listener and accepts
connections.

## Sessions

The node listens over plain TCP — the `dusk_node` binary binds `0.0.0.0:9090`.
For each incoming connection, `init` spawns a **session** that shares the node's
single [namespace](../concepts/namespaces.md), so every connected client sees the
same processes. A session wraps a `DuskServer` as a Cap'n Proto bootstrap
capability and runs an RPC system over the stream, so the client ends up holding
a `Dusk` capability. Once connected, a client can
[drive the node](connect-a-client.md).
