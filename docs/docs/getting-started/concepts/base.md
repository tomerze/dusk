# Base programs

Dusk Base is the set of programs that ship with Dusk - the programs a node can
run out of the box. They're optional and swappable: you can write your own
out-of-tree set and link that into your nodes instead. The relationship between
Dusk Base and Dusk is like the one between GNU Coreutils and the Linux kernel -
you can go without it, but together they make a node feel like a complete system.

A node can run any Base program over `Dusk.process`; most are also invocable from
the [shell](../../features/shell.md) by name, which is how you'll usually reach
them when [diagnosing](../../features/diagnosis.md) a node.

## `sh`

The shell. Runs commands against the node, supports `&&` / `||`, and lets you
define functions. It's what you talk to through the `dusk` CLI. See
[Shell](../../features/shell.md).

## `ps`

Lists the Dusk processes *inside* the node - the programs Dusk is running,
like `docker ps` (not the host's process table) - each with its name, version,
pid, state and program id.

The `stat` column is one or two letters, the way `ps(1)` does it: `R` for
running, `S` for suspended, `Z` for exited, and a leading `R` once the process
has said it is ready. So `RR` is ready and running, `R` running but not yet
ready, `S` suspended - created but not yet run - and `Z` finished and waiting
for a `waitpid` to take it out of the list.

```sh
ps            # list every process on the node
ps 0x1a2b     # show just the process with this pid (hex or decimal)
```

## `programs`

Lists the programs the node can launch - what it was *compiled with*, as opposed
to what is currently running (`ps`) or what this client knows how to invoke
(`help`). A node and a client are built separately, so the two sets can differ,
in membership and in version.

```sh
programs      # list every program the node can launch
```

The node identifies a program by its id and holds no name for it, so the list is
sent back to the client, which names each id from its own shell entries. A
program this client has no entry for shows `N/A`; a program with more than one
entry shows all of them, separated by ` | `. If that client has disconnected by
the time the node has the list, or the command was compiled into the node, the
node sends the list without the `Shell Entry` column.

## `logs`

Reads the node's rolling log buffer - live in an interactive viewer, or streamed
out to a file or a collector. See [Logs](../../features/logs.md).

```sh
logs                            # open the interactive log viewer
logs stream file://out.jsonl    # stream logs to a file
logs stream otlp://myotel:4317 # stream straight to an otel collector (traces included!)
logs dump --replay-only         # return the buffered history as values, then exit
```

## `kvs`

Reads and writes the node's in-memory key-value store, shared by every program
on it. See [Key-value store](../../features/kvs.md).

```sh
kvs get dusk.version         # read a value
kvs get dusk                 # read every key starting with dusk
kvs set deploy.stage canary  # store a string
kvs scan                     # list every key
```

## `kill`

Sends a signal to a process by pid. The default signal is `15` (Terminate),
asking the process to exit.

```sh
kill 0x1a2b              # send Terminate (15)
kill --signal 8 0x1a2b   # send Reap (8), reaping a zombie
kill --signal 7 0x1a2b   # send Sweep (7), clearing a suspended process
kill --signal 9 0x1a2b   # send a different signal
```

Signal `8` is [`Reap`](signals.md#the-signal-type): it reaps a zombie process,
logging its exit result, and does nothing to a non-zombie process. Signal `7` is
[`Sweep`](signals.md#the-signal-type): it takes out a process that was created
and never run, and does nothing to a process that has run.

## `sleep`

Waits for a given number of milliseconds, then exits. A Terminate signal ends it
early, so Ctrl+C at the prompt stops the wait instead of letting it run out.

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

Do nothing and report success (`true`) or failure (`false`) - building blocks for
shell conditionals.

## `init`

The first process started on a node. It is handed an init script - a
`Bytecode.Bytecode` message - and starts a detached `sh` to run it; for the node
artifact the script is by default `nightfall -l 9090`, which starts
[`nightfall`](#nightfall). You don't run `init` by hand - the node starts it at
boot. See [Run a standalone node](../guides/run-a-node.md).

## `nightfall`

Listens on TCP and opens a Dusk session with every connection it accepts. It
keeps running until it is terminated. The node artifact has `init` run it at
boot, on the node's listen address; run from the shell it opens one more
listener and holds the shell as a foreground command until it is terminated.

```sh
nightfall -l 4000              # listen on port 4000, on every address
nightfall -l 127.0.0.1:4000    # listen on one address
```

`ps` shows it as `nightfall[listen :4000]`, or `nightfall[listen 127.0.0.1:4000]`
when it listens on one address. The port is the one it is listening on, so
`nightfall -l 0` shows the port the system picked.
