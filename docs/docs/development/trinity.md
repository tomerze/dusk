# The Trinity Problem

An interactive program — a shell prompt, a log pager — has a session that
somebody has to own. In Dusk three parties each have a legitimate claim on it,
and no design satisfies all three at once. This page names the three, shows how
`logs view` resolves them, what an interactive `sh` needs instead, and what a
first attempt at it got right and wrong.

It describes a design that is **not implemented**. `sh` today has no prompt of
its own: the `dusk` CLI creates one long-lived `sh[server]` process at startup
and drives it for the whole session, so `sh` typed at that prompt, or
`dusk.sh("sh")` from Python, has nothing to open. Making `sh` an ordinary
program that opens a prompt is the change this page is about.

## What it has to do

The behaviour to build towards, in the order it is worth testing:

- **`sh` at a prompt opens a nested prompt**, on a second `sh` process. `exit`
  returns to the prompt that ran it, which keeps working. It nests to any
  depth — `sh` inside `sh` inside `sh` — and each `exit` unwinds one level.
- **`sh; sh` in one script opens one prompt and refuses the second**, saying the
  terminal is already taken. Both statements run; the second reports that it
  cannot have a terminal, the way any program reports that it cannot have what
  it needs.
- **`dusk.sh("sh")` from Python** opens the prompt in the calling process's
  terminal, and the iterator ends when the user leaves it — the same shape as
  `logs view`.
- **A node restart under a live prompt does not end the prompt.** The status
  line reads `disconnected`, commands during the outage fail, and when the node
  answers again the next command runs.
- **Under `DUSK_NON_INTERACTIVE`**, a bare `sh` is refused at args-build time,
  the way `logs view` already is, so the API gateway never spawns a process that
  would wait on a terminal it lacks.

**Refusing the second sibling prompt is a decision, not a limitation.** An
earlier attempt queued it instead — the second prompt opened when you exited the
first. That is strictly more machinery (a wait queue, a resume-ahead-of-queue
rule) for a case nobody asked for, and it produced orderings no one could
predict: with `sh; sh` running and a nested `sh` inside the first, exiting the
nested one handed the terminal to the script's second shell rather than back to
the first. Refuse, and all of that goes away.

## The three claimants

**The node.** A session is a process. It appears in `ps`, it dies to `kill`, and
the statement that started it isn't finished while it runs. The interpreter that
runs `ps; date` has every reason to think the same of `sh; sh`.

**The connection.** A session is a Cap'n Proto call. The client asked the node to
do something and is awaiting the answer; when the answer comes, the thing is
done. Every ordering the node can enforce, it enforces through a call that is
open for the duration.

**The terminal.** A session is whoever is reading stdin. There is exactly one
keyboard, and two readers is not a degraded state — it is *garbage*: keystrokes
are dealt out between them, and nothing typed arrives whole.

Each claim is reasonable. The trouble is that they disagree about lifetime:

- The node's claim ends when the process exits.
- The connection's claim ends when the connection drops — which a node restart
  does, without the session being over in any sense the user recognises.
- The terminal's claim ends when the human stops typing at that prompt, which no
  RPC and no process is in a position to observe.

**You can honour two.** Which two you pick is the whole design, and the third
comes back as a bug.

## How `logs view` resolves it: node + connection

`logs` picks the node and the connection, and gives up surviving a disconnect.

```
main()      open_stream() on the client        ← before `ready`
            client hands back a Stream capability (the pager)
            node streams entries into it
            streamer long-polls stream.stop(); the pager answers when the user quits
            streaming ends → signal streamer_done
output()    streamer_done.wait().await         ← parked for the whole session
            answers daemonize = false          ← at the end
client      Execution sees daemonize == false → kill + waitpid
```

(`base/logs/src/lib.rs`: `main` around line 155, `output` around line 196;
`base/logs/src/streamer.rs:89` is the `stop` long-poll.)

Consequences, all of them good for `logs`:

- **The node sequences.** `logs view; logs view` in one script runs one pager,
  then the other, because statement one does not finish until `output` returns.
  Nothing client-side is needed to arrange that.
- **The terminal is exclusive for free.** Only one pager can be open at a time
  along that path, so nothing has to arbitrate stdin.
- **A node restart ends the session.** The stream capability is on the far side
  of the connection; when it dies, the pager dies. `logs` does not try to
  reconnect, and nothing in its design would let it.

That last line is the price, and for a pager it is the right price.

## Why `sh` cannot copy it

An interactive shell has one requirement `logs view` does not: **the session
must survive the node.** Today's CLI holds its `sh` process behind
`capnp_rpc::auto_reconnect`; a node restart shows `disconnected` in the status
line and the next command runs against a freshly created process. Losing that is
not acceptable, so `sh` cannot let a call own its session — which means giving
up the *connection's* claim, and with it the node's ability to sequence.

Everything the node stops doing, the client has to do:

| What the node does for `logs` | What `sh` must do instead |
|---|---|
| Statement waits for the session | The client waits for prompts the command it just ran opened |
| One session at a time, implicitly | Arbitrate the terminal on the client |
| Process reaped by `Execution` | The prompt owns the process and kills it |

### The terminal, under the refuse rule

There is one holder. A prompt asks for the terminal when it opens; if it is
taken, the prompt refuses and says so, and the `sh` that would have owned it is
killed rather than left running with nobody to drive it.

The one subtlety that survives is nesting, and it is the whole reason a plain
"taken" boolean is not enough. A prompt that is *running a command* must let a
prompt that command opens take the terminal — otherwise the outer prompt holds
it while waiting for the nested one, and the nested one waits for the terminal:
a session that never ends. A prompt that is *reading a line* must not.

So the state is not taken/free but three-valued: free, held-and-reading,
held-and-inside-a-command. A prompt asking finds:

- **free** — take it.
- **held-and-inside-a-command** — this is a nested prompt. Take it, and hand it
  back when it ends.
- **held-and-reading** — this is a sibling. Refuse.

Nothing needs to mark a prompt as nested. "Nested" is what asking for the
terminal looks like while the holder is inside a command; that is the only
difference between the two cases, and the caller does not have to know which it
is.

### Ordering, which the terminal does not give you

Exclusion is not sequencing. Even with the terminal arbitrated, the client still
has to park whoever ran `sh` until the prompt it opened is gone — otherwise the
CLI exits, or Python returns its iterator, or an outer prompt redraws, while a
prompt is still on the terminal. A count of open prompts on the thread, raised
before the prompt's task is spawned and lowered by a guard when it ends however
it ends, is enough: take the count before running a command, wait for it to come
back down after.

Raise it *before* spawning, not inside the task. Between the call and the task
first running there is a window in which the count would read zero and the
caller would carry on.

## What a first attempt got right

An abandoned branch built this. These parts were measured working and are worth
taking:

**The handover.** `ShArgs.Server` gains `serve(process)`. A server-mode `sh`
calls it from `Process::main` the moment it is running — just before `ready`,
which `portal()` waits on by itself — handing over its own `Process`
capability, minted from the process object the way `Dusk.ps` mints its entries
(`Process::clone_box` into `capnp_rpc::new_client`). The client answers the RPC
at once and opens the prompt as a task of its own.

**Do not await `serve` on the node.** The first implementation ran the prompt
*inside* the incoming RPC, so the node blocked until the user exited — node
sequencing, exactly like `logs`. It has to be abandoned: a connection drop takes
the prompt with it, because capnp cannot resume an incoming call on a new
connection. The prompt was torn down mid-`read_line` and the process exited with
the terminal still in raw mode.

**`output` must answer `daemonize = true`.** This is the contract the node reads
to decide whether to kill the process, and a server-mode `sh` now belongs to the
client. Getting it wrong is vicious to debug: the prompt draws, the keepalive
paints a live RTT, and every command fails with `process is not running, cannot
get portal` — and because that is `Failed` and not `Disconnected`,
`auto_reconnect` never rebuilds, so nothing recovers and only Ctrl-D exits.

**Reconnect.** Put the process behind `capnp_rpc::auto_reconnect` and install
the served one as its first incarnation with `set_target`; the factory then only
runs when the node has gone away. The keepalive pings `pid()` and the status
line reads `disconnected` while it fails.

Two traps in that path:

- **The replacement serves itself.** A process the client's own reconnect
  created is server-mode like any other, so it calls `serve` at a client already
  sitting at a prompt for that shell. Taken at face value it opens a second
  prompt. Serving has to be idempotent: record every pid this client holds — the
  served one, and each replacement, registered as it is created so the `serve`
  that follows already finds it held — and open nothing for a pid on the list.
- **The first call after a reconnect fails.** `auto_reconnect` returns the first
  call's `Disconnected` error while it refreshes its capability in the
  background; the next call uses the refreshed one. Retry once when creating the
  replacement, or the failure surfaces to the user as a mysterious first-command
  error.

**`sh -d "sleep 3000; sh"`** is the case that defeats every node-side sequencing
design, including `logs`'s. A detached script is deliberately unattached:
`output` answers `daemonize` immediately, nothing anywhere is waiting on it, and
fifty minutes later it opens a prompt. No call is open to sequence it and no
client-side count knows about it. Under the refuse rule it simply refuses if
somebody is reading, which is the right answer and costs nothing. **If you are
considering moving ordering back into the node, this is the case to answer
first.**

**Nothing arbitrates across processes.** Whatever holds the terminal is
thread-local inside one client. Two `dusk` processes on one tty still fight, as
two `cat`s would. That is out of scope and should stay out.

## Traps that cost time

- **`std::panic` must be imported in `ui/spinner.rs`.** `tokio::select!` expands
  a bare `panic!` at its call site; without the import rustc 1.98-nightly
  resolves it two ways and aborts with an internal compiler error
  ("inconsistent resolution for a macro") instead of reporting the ambiguity.
  Unrelated import changes elsewhere in the crate can mask or expose it.
- **Running `Execution` on a client needs `critical-section` with `std`.** On a
  node that comes from the nix impl; a client has to declare it.
- **Every crate enabling `dusk_program_sh/client` now compiles the prompt
  stack** — reedline, nu-\*, termimad, `dusk_llm` — including the test crates.
  Build time only, but it is noticeable.
- **Clippy's `module_inception` rejects `prompt::prompt`**, so the prompt's
  entry file is `client/prompt/mod.rs`.

## Solutions that were on the table

**Park `output` until the shell ends** (the `logs` shape, without putting the
prompt inside a call). Buys node sequencing and deletes the client-side count.
The objection is not cost, it is that a held-open `output` is the node's grip on
a session required to outlive the node: at the first disconnect the call fails,
the statement fails, and the client must decide what to do about a prompt that
is still alive — which is the count again, plus a new failure mode. And
`sh -d "sleep 3000; sh"` still escapes it.

**Give up reconnect.** Then `sh` becomes `logs view` and this page is one
paragraph long. It is a real option if the shell's survival across node restarts
is ever judged not worth its cost.

## Why "the Trinity Problem"

Three claimants, one life, and you may honour two. `logs view` takes the node and
the connection and lets the terminal look after itself, because only one pager
can ever be open. `sh` takes the terminal and survival, and rebuilds ordering on
the client because the connection can no longer be trusted to carry it. The
combination nobody can have is all three: a session the node sequences, that a
call owns, that also outlives the call.

Every bug in this area has been the third claimant coming back for its due —
garbled input when the terminal was ignored, a prompt torn down mid-keystroke
when the connection was, an orphaned `sh[server]` when the node was.
