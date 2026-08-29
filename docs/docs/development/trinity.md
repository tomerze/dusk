# The Trinity Problem

An interactive program — a shell prompt, a log pager — has a session that
somebody has to own. In Dusk three parties each have a legitimate claim on it,
and no design satisfies all three at once. This page names the three, shows how
`logs view` resolves them, how `sh` resolves them differently and why, and walks
the cases that break each resolution.

Read it before changing anything about `ShArgs.Server.serve`, `wait_serving`, the
terminal token in `base/sh/src/client/prompt/`, or the `daemonize` a server-mode
`sh` answers `output` with. Those four are one mechanism, and each exists
because of a different claimant.

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
comes back as a bug in the cases at the end of this page.

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

## How `sh` resolves it: terminal + survival

An interactive shell has one requirement `logs view` does not: **the session must
survive the node.** On `master` the CLI held one long-lived `sh` process behind
`capnp_rpc::auto_reconnect`; a node restart showed `disconnected` in the status
line and the next command ran against a freshly created process. Losing that was
not acceptable, so `sh` cannot let a call own its session.

So `sh` gives up the *connection's* claim, and with it the node's ability to
sequence:

```
Process::main   serve(process) on the client   ← before `ready`; portal() waits on Ready itself
                (client answers at once — fire and forget)
output()        answers daemonize = true       ← the daemonise signal
client          Execution sees daemonize == true → does not kill, moves on
client task     Shell::adopt(process) → take the terminal → Prompt::run()
                exit → shell.kill() → process dies
```

The node is told, in the only vocabulary it has, *this one is not mine any
more*. `base/sh/src/lib.rs` (`main`, `Portal::output`),
`base/sh/src/client/prompt/mod.rs` (`serve`, `serve_prompt`),
`base/sh/src/client/shell/mod.rs` (`adopt`, `kill`).

Everything the node stopped doing, the client now has to do, and that is what
the extra machinery is:

| What the node did for `logs` | What `sh` does instead |
|---|---|
| Statement waits for the session | `wait_serving()` — a prompt (and the CLI, and `dusk_py`) waits for prompts opened by the command it just ran |
| One session at a time, implicitly | The terminal token — one holder, a queue for the rest |
| Process reaped by `Execution` | The prompt owns the process and kills it (`Shell::kill`) |
| — | `Shell::adopt` is idempotent: a replacement process the shell made itself is recognised by pid and opens no second prompt |

### The terminal token, precisely

`base/sh/src/client/prompt/mod.rs`. One thread-local `TERMINAL`; each prompt is
given a number when it asks.

- **take** — free? take it. Held? join the back of the queue.
- **lend** — a prompt entering a command marks the terminal free *without*
  waking the queue. A prompt the command opens can take it; prompts already
  waiting stay parked, because they are waiting for this prompt to be *done*,
  not for it to be *busy*.
- **resume** — after the command, ask again, ahead of the queue.
- **release** (guard drop) — hand it to the first in the queue, or free it. A
  prompt that ends while it had lent the terminal out passes on nothing: the
  prompt that borrowed it is the holder and will hand it on itself.

Lending is what makes nesting possible. Without it the outer prompt would hold
the terminal while waiting for the nested one, and the nested one would wait for
the terminal: a dead session, forever.

### There is no nesting mechanism

Worth stating plainly, because it looks like there should be one. Nothing marks a
prompt as nested. "Nested" is what you get when a prompt asks for the terminal
while the holder is inside a command; "queued" is what you get when it asks while
the holder is reading. The caller cannot tell you which it is, and does not have
to.

Nor does the command park the outer prompt. `shell.sh("sh")` returns almost
immediately — the nested `sh` daemonises, so the interpreter finishes the
statement and sends `done`. What holds the outer prompt is the line after it:

```rust
self.terminal.lend();
let result = self.shell.sh(line, …).await;   // returns at once
wait_serving(serving_before).await;          // ← this is the parking
self.terminal.resume().await;
```

Ordering comes from `wait_serving`; exclusion comes from the terminal. Two jobs,
two mechanisms, and the cases below need both.

## The cases

**`sh`** — one prompt. The CLI runs `sh` in server mode, the process serves
itself, the prompt opens, `exit` kills it, the CLI leaves.

**`sh; sh` (as a script)** — the interpreter runs both statements back to back,
because each daemonises. Two prompts are created within milliseconds. Nothing
sequences them: the CLI's `wait_serving` is outside the whole script, and the
node has no such notion. The terminal is the only thing standing between you and
two readers. *Before the token existed this was observable garbage:* typing
`echo who` produced `eh h` at one prompt and `cowo` at the other. With the
token: one prompt, and the second opens when you exit the first.

**`sh` typed at a prompt** — the outer prompt lends the terminal, its RPC returns
at once, `wait_serving` parks it. The nested prompt finds the terminal free and
takes it. `exit` kills the nested `sh`, the terminal is released, `wait_serving`
completes, the outer prompt resumes.

**`sh; sh` typed at a prompt** — the same, twice: the first nested prompt takes
the terminal, the second queues behind it, the outer prompt's `wait_serving`
waits for both.

**`sh -d "sleep 3000; sh"`** — the case that defeats every sequencing design,
including `logs`'s. A detached script is deliberately unattached: `output`
answers `daemonize` immediately, nothing anywhere is waiting on it, and fifty
minutes later it opens a prompt. No call is open to sequence it and no
`wait_serving` counted it. Only the terminal token saves this: the late prompt
queues behind whoever is reading, and you land in it when you exit them. **If you
are considering moving ordering back into the node, this is the case to answer
first.**

**Node restart while a prompt is up** — the prompt is not inside any call, so it
survives. Its `Shell` builds a replacement process through `auto_reconnect`;
that process, being server-mode, serves *itself* on start. `Shell::adopt` sees a
pid it already holds and opens nothing — this is why serving must be idempotent.
Meanwhile the status line reads `disconnected`, and the next command runs when
the node answers again.

**`dusk.sh("sh")` from Python** — identical path; the prompt opens on the calling
terminal, and the iterator ends when it exits. Under the API gateway, which sets
`DUSK_NON_INTERACTIVE`, a bare `sh` is refused at args-build time instead, the
way `logs view` is.

**A prompt that dies mid-command** — panic, or its task dropped. The count is an
RAII guard, so `wait_serving` cannot hang on it; the terminal guard checks
whether it is still the holder before handing anything on, so a prompt that died
while lending passes on nothing.

## Solutions that were on the table

**Await the `serve` call** (the first implementation). The prompt runs *inside*
the incoming RPC, so the node blocks until you exit — node sequencing, exactly
like `logs`. Rejected because a connection drop takes the prompt with it: capnp
cannot resume an incoming call on a new connection, the prompt was torn down
mid-`read_line`, and the process exited with the terminal still in raw mode.

**Do not daemonise: park `output` until the shell ends** (the `logs` shape,
without putting the prompt inside a call). Buys node sequencing and deletes
`wait_serving`. The handshake is five lines — `logs` has it as `streamer_done`.
The objection is not cost, it is that a held-open `output` is the node's grip on
a session required to outlive the node: at the first disconnect the call fails,
the statement fails, and the client must decide what to do about a prompt that is
still alive — which is `wait_serving` again, plus a new failure mode. And
`sh -d "sleep 3000; sh"` still escapes it. *Once the prompt must outlive the
connection, the client owns the waiting; once the client owns the waiting, node
sequencing adds ordering only while nothing goes wrong.*

**Refuse the second prompt.** Simple and honest, and it makes `sh; sh` an error
rather than two shells. Rejected as a worse answer to a script that is not
obviously wrong.

**A registry instead of idempotency** — the client tracks which processes it
serves and refuses unknown ones. This is what the pid set in `Shell` does today,
minimally; a fuller registry would also let a client refuse to be served at all.

**Give up reconnect.** Then `sh` becomes `logs view` and this page is one
paragraph long. It is a real option if the shell's survival across node restarts
is ever judged not worth its cost.

## Known residue

- **Ordering between a queued prompt and a resuming one.** Script `sh; sh`, and
  you type `sh` inside the first: exiting the nested prompt hands the terminal to
  the queue — the script's second shell — before the first shell resumes. Single
  reader throughout, but not the order you would guess. Fixing it means the
  release path knowing that a lender is waiting to resume.
- **Two nested siblings and a queued one** interleave by the same rule.
- **Nothing arbitrates across processes.** The terminal token is thread-local
  inside one client. Two `dusk` processes on one tty still fight, as two `cat`s
  would.

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
