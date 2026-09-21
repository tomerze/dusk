# Run a dusk node

A node is one running Dusk server - one machine in your fleet. This guide covers
running one beyond the quickstart binary.

## The `dusk_node` binary

A node ships as **`dusk_node`** - the server binary under `artifacts/`, run with
`cargo run --bin dusk_node` (its `main` just passes its argument, if it has one,
to `dusk_node::dusk_node_run()`).
It links the portable core, a set of programs, and one impl - by default the
**nix** impl (`impls/nix/`, a library, not a binary) for Unix-like systems. You make a node
your own by linking in your programs and, if needed, swapping the underlying impl.
See [Build a custom impl](custom-impl.md).

## Choosing the listen address

`dusk_node` takes one optional argument - the `ip:port` it listens on. With no
argument it binds `0.0.0.0:9090`, every interface:

```bash
cargo run --bin dusk_node                       # 0.0.0.0:9090
cargo run --bin dusk_node -- 127.0.0.1:9090     # loopback only
cargo run --bin dusk_node -- '[::1]:9090'       # IPv6 loopback
```

The argument is a literal IP address and a port; a hostname is not resolved. An
argument that is not one exits with status `64` (`EX_USAGE`) without starting the
node.

## What happens at startup

Bringing a node up follows a fixed sequence:

1. Create the node's [namespace](../concepts/namespaces.md) with a random id and
   the executor's spawner.
2. Register the impl's [`LauncherSet`](../concepts/launchers.md) against that
   namespace - this is the set of programs the node can run.
3. Spawn the first process, `init`, into the namespace via an in-process `Dusk`
   client (`Dusk.process` + `Dusk.run`).

The `init` process is handed an init script - a `Bytecode.Script` - and runs it
through `sh` in Script mode. For the node artifact that script is `nightfall -l
9090`, compiled at build time, or `nightfall -l <ip:port>` lowered at run time
when the node is given an address. [`nightfall`](../concepts/base.md#nightfall)
binds the node's network listener and accepts connections, running in the
foreground of that script.

## Sessions

The node listens over plain TCP, on the address
[chosen at startup](#choosing-the-listen-address).
For each incoming connection, `nightfall` spawns a **session** that shares the node's
single [namespace](../concepts/namespaces.md), so every connected client sees the
same processes. A session wraps a `DuskServer` as a Cap'n Proto bootstrap
capability and runs an RPC system over the stream, so the client ends up holding
a `Dusk` capability. Once connected, a client can
[drive the node](connect-a-client.md).
