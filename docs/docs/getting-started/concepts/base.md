# Base programs

Dusk Base is the set of programs that ship with Dusk — the commands a node can
run out of the box. They're optional and swappable: you can write your own
out-of-tree set and link that into your nodes instead. The relationship between
Dusk Base and Dusk is like the one between GNU Coreutils and the Linux kernel —
you can go without it, but together they make a node feel like a complete system.

A node can run any Base program over `Dusk.process`; most are also invocable from
the [shell](../../features/shell.md) by name, which is how you'll usually reach
them when [diagnosing](../../features/diagnosis.md) a node.

## `sh`

The shell. Runs commands against the node, supports `&&` / `||`, and lets you
define functions. It's what you talk to through the `dusk` CLI. See
[Shell](../../features/shell.md).

## `ps`

Lists the Dusk processes running *inside* the node — the programs Dusk is
running, like `docker ps` (not the host's process table) — each with its pid,
name, version, and program id.

```sh
ps            # list every process on the node
ps 0x1a2b     # show just the process with this pid (hex or decimal)
```

## `kill`

Sends a signal to a process by pid. The default signal is `15` (Terminate),
asking the process to exit.

```sh
kill 0x1a2b              # send Terminate (15)
kill --signal 9 0x1a2b   # send a different signal
```

## `sleep`

Waits for a given number of milliseconds, then exits. A Terminate signal ends it
early.

```sh
sleep 500     # sleep 500 ms
```

## `date`

Shows the node's wall-clock time, or sets it.

```sh
date                              # show the node clock
date -s "2026-05-30 21:00:00"     # set it to a UTC timestamp
date --ntp pool.ntp.org           # set it from an NTP server (host[:port])
date --sync                       # set it to the connecting client's clock
```

## `hostname`

Prints the node's hostname.

## `true` / `false`

Do nothing and report success (`true`) or failure (`false`) — building blocks for
shell conditionals.

## `init`

The first process started on a node. It accepts incoming client connections and
spawns a session for each. You don't run `init` by hand — the node starts it at
boot. See [Run a standalone node](../guides/run-a-node.md).
