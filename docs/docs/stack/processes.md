# Processes: the unit of work

This page is for the people who run the Dusk stack and for anyone who drives
nodes through it. It says how work runs on a node: at which pid, how a node that
already has the work is recognised so nothing runs twice, what each status in
`dusk.process-results` means, and when a finished process is cleaned up.

Work sent to a node - a campaign's script, a facts read, a log stream, a file
collection, an interactive session - is a **process** on the node, identified by its pid, and running it once is a
property of Dusk itself: a node's namespace holds one process per pid. The
stack adds nothing to the node for this; dawn drives the node's own `sh` program
through the `dusk` Python extension, as any client can.

## The pid

twilight picks the pid before anything reaches the node, and records it first
(see [Intended processes](#intended-processes)).

* **Campaign work** runs at a pid derived from the campaign, the node and the
  attempt: the first 8 bytes, big-endian, of
  `SHA-256("dusk-pid-v1" || campaign id || "/" || device id || "/" || installation id || "/" || attempt)`.
  A pid in the reserved range - below 65536, or a pid a schema names, such as
  the default shell's - is replaced by hashing again with `/1`, `/2` and so on
  appended. Every resend of one attempt, from any twilight instance, sends the
  same pid; a new attempt gets a new pid.
* **Everything else** - facts reads, file collections, log streams,
  interactive sessions - runs at a random pid, in a shell at that pid. dawn
  kills and reaps that pid itself as soon as the work is done.

## The default shell

Every node runs a shell server at a fixed pid, the `defaultPid` that
`sh.capnp` names, which serves any number of `sh` calls at once. A client that
connects without naming a pid - `dusk.Dusk(address, port)` - talks to it. dawn
runs everything that is not the work itself there: `ps`, reads (`kvs get`,
`logs dump`), kills and reaps. It starts no other shells.

twilight holds those commands to a budget. Every intended process (below)
allows a number of default-shell commands, set by twilight's
`reconcile.default_shell` configuration: for a campaign's process the `ps`, the
reads of its reported state, one `logs dump` and the kill; for any other work
the kill and the reaps that release its pid, and for a reap one more per pid it
reaps. Reconcile counts the default-shell commands on each node: one while none
of the node's intended processes is open raises `default_shell_without_intent`
(critical), and more than the open processes' budgets together raises
`process_shape` (high).

## Running work once

For each piece of work, dawn:

1. **Looks for the pid.** It runs `ps` in the default shell. If the pid is
   there - running, or exited and not yet reaped - the work was dispatched
   before: dawn reports `duplicate`, and **nothing runs again**.
2. **Starts a shell at the pid.** Otherwise `dusk.Dusk(address, port,
   sh_server_pid=<pid>, ...)` puts a shell server at that pid. As soon as it is
   up dawn reports `started`, and `.sh(script)` runs the script in it; the
   script's output streams back to dawn, which writes it to
   `dusk.process-output`.
3. **Finishes the work.** When the script succeeded or failed, dawn reads the
   reported version or configuration in the default shell, and runs one `cp`
   per file to collect and one `logs stream` in the shell at the pid.
4. **Kills the shell.** When the script succeeded, failed or `ended`, dawn
   kills the shell at the pid from the default shell. The exited process stays
   in the node's process table, unreaped, as the marker that the work was done;
   it holds none of the node's process slots. After any other outcome - the
   script still `running`, a time-out, an error once the shell was up - the
   shell at the pid is left as it is until twilight has the pid reaped.
5. **Reports the result** to `dusk.process-results`.

A node keeps its process table in memory. **A node that restarted has an empty
one**, so work sent again after a restart is not found and runs again. That is
why dawn reads an `ensure_version` node's reported version before it runs
anything. A one-shot action - `run_script`, `quarantine` - is sent again, at
the same pid, only while nothing says it reached the node; once it started, a
missing result is left to an operator and never run again. A send whose fate
was unknown, followed by a restart of the node before the resend, can still run
a one-shot script twice.

## Statuses

Every piece of work ends in one `dusk.process-results` message with one of these
statuses; `started` comes first, as soon as the shell at the pid is up.

| Status | Meaning |
|--------|---------|
| `started` | dawn started the shell server at the pid and is sending it the script. The final status follows. |
| `succeeded` | The script ran to its end without an error. |
| `failed` | The script ended with an error. |
| `duplicate` | The pid was already in the node's process table: the work was dispatched before, and nothing ran now. |
| `running` | The stream to the node broke after `started`, and the node's logs show the script has not ended. |
| `ended` | The stream broke after `started`, and the node's logs show the script ended; how it ended is not in the logs. |
| `already_satisfied` | An `ensure_version` node already reported the desired version; nothing ran. |
| `unreachable` | dawn could not reach the node: it is not connected, or the connection failed. |
| `timed_out` | The work took longer than its time-out. `delivered` says whether the shell at the pid was started. |
| `denied` | nightfall refused a call the work needed: the principal's roles do not allow it. |
| `reaped` | The pid was killed and reaped at twilight's request. |
| `error` | Anything else went wrong; the message's `error` says what. |

`delivered` is true when this dispatch started the shell server at the pid. What
twilight makes of each status is on [Campaigns](campaigns.md#each-nodes-state):
`duplicate` on a one-shot row means an earlier send was delivered; `ended` on a
one-shot row becomes `unknown` and is never run again, while on an `ensure_*`
row, and on a quarantine that does not require its script's success, twilight
verifies the reported state instead.

### When the stream breaks

When the stream from the node breaks after `started`, dawn reads the node's log
buffer from the default shell (`logs dump --replay-only`) and looks for the end
of the shell's `sh_exec` span for the pid: no end means `running`, an end means
`ended`. The logs say that the script ended, not whether it succeeded.

## Reaping

The exited process at a pid stays in the node's process table until it is
reaped, and as long as it is there a resend of the same pid is a `duplicate`.
twilight therefore asks dawn to reap a pid only once nothing can send it again:
its campaign row is finished, or has moved on to a later attempt, and the
campaign's retry horizon - its longest backoff plus the node time-out - has
passed. dawn kills and reaps the pid from the default shell (`kill <pid>`, then
`kill --signal 8 <pid>`, the [`Reap`](../getting-started/concepts/signals.md#the-signal-type)
signal) and reports it `reaped`. The pid of other work - a facts read, a file
collection, a log stream, an interactive session - dawn kills and reaps itself
as soon as the work is done; twilight's sweep asks for it too once its intended
process has expired, which finds it gone unless dawn could not reap it. A node
that restarts reaps everything at once, by forgetting it.

## Intended processes

Before every call to dawn that reaches a node - dispatch, reap, facts, logs,
files, interactive sessions - twilight records the pid in its
`intended_processes` table, and commits it before the call: the pid, the device and installation, the campaign and attempt when there is
one, the kind of work, who asked for it (`campaign:<id>` or the operator), until
when it is intended, how many shell commands may run in its own shell - for a
campaign's process its script, one `cp` per collected file and one
`logs stream` - and how many it allows in the node's default shell
([The default shell](#the-default-shell)).

That table is what the ledger is checked against. nightfall records every call
it forwards with the pid of the process the call works under - the pid a
`Dusk.process` names, the pid a `Dusk.kill` or `Dusk.waitpid` names, or the pid
of the process every capability of the call descends from - and twilight's
reconcile raises an alert for a process at a pid twilight never intended, a pid
on the wrong node, calls after a process's deadline or its result, and more
shell commands than a process was intended to run, in its own shell or in the
default shell ([Reconcile alerts](campaigns.md#reconcile-alerts)). Processes a
client starts without naming a pid belong to no intended process and are only
counted; the [security model](security.md#what-stays-undetectable) says what
that, and the content of default-shell commands within their budget, leaves
open.
