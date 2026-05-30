# Base

Dusk Base is the set of programs that ship with Dusk — the core utilities you'd
expect on a node. They are optional and swappable: you could write your own
out-of-tree set and link that into your nodes instead. The relationship between
Dusk Base and Dusk is like the one between GNU Coreutils and the Linux kernel —
you can go without it, but together they make a node feel like a complete system.

A node can run any Base program over `Dusk.process`; most are also invocable from
the [shell](../../features/shell.md) by name.

## Programs under Base

### `sh`

The shell. Runs commands against the node. It's what you talk to when you connect
with the `dusk` CLI. See [Shell](../../features/shell.md).

### `ps`

Lists the Dusk processes running inside the node — the programs Dusk is running,
like `docker ps` (not the host's process table) — with each one's pid, name,
version, and program id.

### `kill`

Sends a signal to a process on the node by pid. By default it sends signal `15`
(Terminate), asking the process to exit.

### `sleep`

Waits for a given number of milliseconds on the node, then exits. A Terminate
signal ends it early.

### `date`

Shows the node's current wall-clock time, or sets it — from a literal Unix
timestamp, an NTP server, or the client's own clock.

### `hostname`

Prints the node's hostname.

### `true` / `false`

Two programs that do nothing but report success (`true`) or failure (`false`).
They're building blocks for shell conditionals.

### `init`

The first process started on a node. It binds the node's network listener and
accepts incoming client connections, spawning a session for each. You don't run
`init` by hand — the node starts it at boot. See
[Run a dusk node](../guides/run-a-node.md).
