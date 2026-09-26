# Run a dusk node

A node is one running Dusk server - one machine in your fleet. This guide covers
running one beyond the quickstart binary.

## The `dusk_node` binary

A node ships as **`dusk_node`** - the server binary under `artifacts/`, run with
`cargo run --bin dusk_node` (its `main` just calls
`dusk_node::dusk_node_run()`).
It links the portable core, a set of programs, and one impl - by default the
**nix** impl (`impls/nix/`, a library, not a binary) for Unix-like systems. You make a node
your own by linking in your programs and, if needed, swapping the underlying impl.
See [Build a custom impl](custom-impl.md).

## The listen address

The listen address is set when the node is built: `dusk_node` takes no
arguments, and built with the default init script it listens on port `9090`, on
every address. Moving it means building with another init script - see
[`DUSK_NODE_INIT_SCRIPT`](../../embedding/node-artifacts.md#dusk_node_init_script).

## What happens at startup

Bringing a node up follows a fixed sequence:

1. Create the node's [namespace](../concepts/namespaces.md) with a random id and
   the executor's spawner.
2. Register the impl's [`LauncherSet`](../concepts/launchers.md) against that
   namespace - this is the set of programs the node can run.
3. Spawn the first process, `init`, into the namespace via an in-process `Dusk`
   client (`Dusk.process` + `Dusk.run`).

The `init` process is handed an init script - a `Bytecode.Bytecode` message - and starts
a detached `sh` to run it, then waits for its own `Terminate`, reaping that `sh`
with `waitpid` when it exits. When the script is one program, that `sh` exits as
soon as it has started the program. For the node
artifact that script is by default the single command `nightfall -l 9090`, compiled
while the artifact itself is built, so nothing is compiled at boot, and `ps`
shows `init` and `nightfall` with no `sh` between them.
[`nightfall`](../concepts/base.md#nightfall)
binds the node's network listener and accepts connections, running in the
foreground of that script.

## Sessions

The node listens over plain TCP, on the address
[set when it is built](#the-listen-address).
For each incoming connection, `nightfall` spawns a **session** that shares the node's
single [namespace](../concepts/namespaces.md), so every connected client sees the
same processes. A session wraps a `DuskServer` as a Cap'n Proto bootstrap
capability and runs an RPC system over the stream, so the client ends up holding
a `Dusk` capability. Once connected, a client can
[drive the node](connect-a-client.md).
