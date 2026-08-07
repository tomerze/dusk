# Logs internals

How the `logs` subsystem works under the hood: the in-memory buffer at its core,
the `BufferLayer` that fills it, and the client streams and viewer that drain it.
For *using* `logs`, see the [Logs page](../features/logs.md).

The core is the buffer — an **in-memory ring of recent signals** built for a node
where **tens of writer threads** and **tens of readers** touch it at once. It
never blocks a writer, it keeps the newest signals per severity, and any number
of readers can tail it without consuming it for the others.

The structure — writers, eviction, readers — is **lock-free**: no mutex guards
the rings on the write or the read path. The one qualification: each write ends
by waking any parked readers, which briefly takes a critical-section lock around
the waker list (on the Linux impl, a global mutex held for a few instructions).

## The shape

The buffer is a set of **lanes**. A lane is an independent ring with its own
budget; one or more severities route to it. The headline property falls out of
this directly:

> A lane only ever evicts **its own** oldest signals. A `TRACE` flood wraps the
> `TRACE` lane and **cannot** touch your `ERROR`s. Per-level isolation is
> structural, not a policy that can be miscomputed.

Signals carry a **global monotonic sequence number** so the lanes can be merged
back into one chronological stream on read.

## Owner, writers, readers

One `SignalBuffer` **owns** the lanes. It does not write or read itself — it hands
out two kinds of handle, each sharing the same lanes:

- `buffer.writer()` → a **`Writer`**. Call it as many times as you have producers;
  every `Writer` appends to the same lanes, lock-free. More writers means more
  `writer()` calls, **not** cloning the buffer.
- `buffer.reader(start, dusk_client)` → a **`Reader`**. Each reader keeps its own
  per-lane cursors and reads non-destructively, so any number of them tail the
  same lanes independently.

`SignalBuffer` itself is a cheap **pointer-clone**: clones share the same lanes, so
an owner can keep one clone and move another into (say) the logs program's
launcher, and both still mint handles. A second, *independent* buffer is always
an explicit `SignalBuffer::new`.

## The files

`base/logs/src/buffer/` is split by concern:

| File | Role |
|------|------|
| `mod.rs` | module root: the `SignalBuffer` owner, `new`/`writer`/`reader`/`reader_with_offset`/`drop_counts`, `StartPosition`, the per-lane routing table, and the shared constants (`LEVELS`, `level_index`, `LANE_SEQUENCE_WRITING`). Re-exports `SignalBuffer`, `Writer`, `Reader`, `StartPosition`, `DropCounts`. |
| `lane.rs` | the data structures — `Lane` (its two rings), `Ring<T>` (`capacity` + `data` + `head` + `tail`), `Descriptor` (the seqlock-guarded slot) — plus `Lane::from_config`, which builds a lane's rings from a `LaneConfig`. |
| `notify.rs` | the readers' wakeup: a version counter plus the parked readers' wakers — any number of readers can park. |
| `io/writer.rs` | `Writer` and its `write` — the lock-free producer path. |
| `io/reader.rs` | `Reader` and its `read`/`try_read` — the merged, non-destructive consumer. |

Enrichment — turning a stored signal into an OTLP log record or span with its
severity and real timestamps — lives one level up in `src/enrich.rs`, called
from the read path. The ingest side lives beside it: `src/tracing/layer.rs`
(always compiled) holds `BufferLayer`, the `tracing` subscriber that captures
events into the buffer.

Sizing lives in `base/logs/src/config.rs`: `LogsConfig { lanes: Vec<LaneConfig> }`,
each `LaneConfig { levels, byte_capacity, signal_capacity }`.

## A lane is two rings (the printk shape)

Each lane is **two fixed allocations**, the layout the kernel's `printk` ring uses:

- a **descriptor ring** — `signal_capacity` small entries, each stamping one
  signal (its position, sequence, level, and where its bytes are);
- a flat **byte arena** — `byte_capacity` bytes, where the signals' bytes are
  copied end-to-end, wrapping around.

Both are the same `Ring<T>`: a `capacity`, a boxed slice of `T` (`Descriptor` for
the descriptor ring, `AtomicU8` for the arena), and a monotonic `head`/`tail`
pair. This is the design's whole point: **there is no per-signal allocation and
nothing to free**. A signal's bytes are *copied into* the arena; "freeing" them is
just a tail position moving forward, after which later signals overwrite those
bytes in place. No allocator on the write path, no garbage collector, no reference
counting — two rings of memory and some atomic counters. The arena is `AtomicU8`,
so a reader copying a signal can race a producer overwriting those same bytes with
no data race; the seqlock below tells the reader whether the copy it took was clean.

The atomics come from [`portable-atomic`](https://crates.io/crates/portable-atomic),
so the 64-bit counters (and the buffer's `Arc`) also work on targets without
native wide atomics — a CAS-less MCU polyfills them via its critical section.

## Lanes and routing

A lane is configured, not hard-coded. `LaneConfig` says *which levels* go to it,
its **byte capacity**, and its **signal capacity**. The default is one lane per
level, but you can group them:

```rust
LogsConfig {
    lanes: vec![
        // ERROR and WARN share a lane: they evict each other to stay in budget,
        // but a TRACE flood in another lane can never touch them.
        LaneConfig { levels: vec![Level::ERROR, Level::WARN],
                     byte_capacity: 1 << 20, signal_capacity: 8192 },
        LaneConfig { levels: vec![Level::INFO],  byte_capacity: 1 << 18, signal_capacity: 4096 },
        LaneConfig { levels: vec![Level::DEBUG], byte_capacity: 1 << 18, signal_capacity: 4096 },
        LaneConfig { levels: vec![Level::TRACE], byte_capacity: 1 << 18, signal_capacity: 4096 },
    ],
}
```

`SignalBuffer::new` builds one `Lane` per `LaneConfig` (`Lane::from_config`) and routes
each level to the **first lane that lists it** (`level_to_lane: [Option<usize>; 5]`).
Levels grouped into one lane share its budget and evict each other; a level listed
by **no** lane is dropped — `Writer::write` returns early, before it even
serialises. Because a lane can hold several levels, the signal's level travels in
the **descriptor** (not implied by the lane), so a reader can recover the severity
on the way out.

### Construction fails loudly on a bad lane

A lane's capacities index its rings with `position % capacity`, so a `0` capacity
would divide by zero. Rather than silently clamp a nonsense config up to `1`,
`Lane::from_config` **errors** on a zero `signal_capacity` or `byte_capacity`, and
`SignalBuffer::new` returns that error (`anyhow::Result`, tagged with the offending
lane index). Bad sizing is surfaced at construction, never papered over.

## A lane's two bounds

Each lane keeps the newest signals within **both** bounds independently, and each
is just a `head`/`tail` pair on the corresponding ring:

- **signals** — the descriptor window `[descriptor_tail, descriptor_head)` never
  exceeds `signal_capacity`.
- **bytes** — the arena window `[data_tail, data_head)` never exceeds
  `byte_capacity`.

Whichever binds first wins; a signal stays readable only while **both** its
descriptor and its bytes are still in their windows. Both tails advance by a plain
`fetch_max(head - capacity)` — monotonic, exact, with nothing to reserve or leak.
Reads never touch either tail: they are non-destructive.

## The write path — never blocks

`Writer::write` is lock-free and safe from many threads at once:

| Step | What happens |
|------|--------------|
| route | `level → lane`; if no lane, drop and return |
| stamp | `global_sequence.fetch_add(1)` — the signal's place in the merged timeline |
| claim | `data_head.fetch_add(size)` and `descriptor_head.fetch_add(1)` — a disjoint byte run and a descriptor slot |
| reclaim | advance both tails past the slot and byte range about to be reused, **before** touching them, then a `Release` fence |
| write | `lane_sequence = WRITING`, a `Release` fence, fill the descriptor, another `Release` fence, copy the bytes into the arena, publish `lane_sequence = position` |

Eviction is **drop-oldest**: the producer never waits for a slow reader. Because
the tails are advanced and fenced *before* the overwrite, a reader still copying
an evicted signal is guaranteed to see the advance on its post-copy re-check and
reject — so there is nothing to reclaim and no reader can be handed torn bytes.

## The seqlock: a consistent read without a lock

The whole read side rests on one field in each descriptor — `lane_sequence` — and
one classic trick: a **seqlock** (sequence lock). A seqlock lets a reader copy data
a writer may be changing *concurrently*, with no lock and no blocking on either
side, by wrapping the data in a counter the reader checks **before and after** it
reads. If the counter moved, the read was racing a write, so the reader throws its
copy away and retries. Readers never block writers; writers never wait for readers.

Here that counter is `lane_sequence` — the **per-lane signal number** the slot
currently holds (`0, 1, 2, …` counting every signal this lane ever took) — and it
carries **two meanings at once**:

- **Identity.** When a slot is settled, `lane_sequence` is the signal number whose
  signal currently occupies the slot. A reader walking signal number `p` accepts
  the slot only when `lane_sequence == p` — that single equality answers both *"is
  this the signal I'm looking for?"* and *"is the slot settled, not mid-write?"*.
- **In-progress sentinel.** While a writer is filling a slot it sets
  `lane_sequence = WRITING` — a value no real signal number can take (`u64::MAX`).
  Any reader that sees `WRITING` knows a write is underway and waits.

The two sides run the protocol in mirror order — the writer publishes last, the
reader checks twice (the writer's Release fence pairs with the reader's Acquire):

```text
WRITER:  lane_sequence ← WRITING          (slot closed — readers see "mid-write")
WRITER:  Release fence  ─┐                (orders WRITING ahead of the fields)
WRITER:  fill seq, level, data_position, data_length
WRITER:  Release fence  ─┼┐
WRITER:  copy the payload into the arena
WRITER:  lane_sequence ← p                (published — the slot is now readable)
         ──────────────────────────────────────────────────────────────────────
READER:  load lane_sequence;  == p ?      no → wait / retry (mid-write or stale)
READER:  read seq, level, data_position, data_length
READER:  Acquire fence  ─┘                (pairs with the writer's first fence)
READER:  re-load lane_sequence still == p ?   no → retry (the fields were torn)
READER:  data_position ≥ data_tail ?      no → reclaimed, skip it
READER:  copy the bytes out of the arena
READER:  Acquire fence  ──┘               (pairs with the writer's second fence)
READER:  re-load lane_sequence still == p,  and  data_position ≥ data_tail ?
READER:  → both hold: accept              else: discard & retry
```

The reader's **second** `lane_sequence` check is the point: if a writer reused the
slot during the copy, `lane_sequence` is no longer `p` (it is `WRITING`, or the next
signal number), the check fails, and the half-copied bytes are dropped — a torn
signal is never returned. This is the LMAX-Disruptor "published sequence" guard.

**Why the fences.** Publishing `lane_sequence` is a `Release` store and the reader's
accept is an `Acquire` load; paired, they guarantee a reader that sees
`lane_sequence == p` also sees every descriptor and byte store the writer made
*before* it published. The fence pairs guard the two windows that store alone
cannot. The **first** writer fence sits between the `WRITING` store and the field
stores: a `Release` store orders only what *precedes* it, so without the fence the
new field values could become visible before `WRITING` — and a reader lapped a
full ring behind could read the overwriter's `data_position`/`data_length` under
the slot's stale published number and act on torn fields. The fence pairs with
the reader's post-field-read `Acquire` fence: a reader that saw any new field is
guaranteed to see the slot move on its re-check, and retries. The **second**
fence pair guards the **byte arena** the same way — it orders the producer's
`data_tail` advance ahead of the overwrite, so a reader that copied a byte the
producer then reclaimed sees the advanced `data_tail` on its re-check
(`data_position < data_tail`) and rejects. That is the
[reclamation gate](#the-write-path-never-blocks) from the write path, seen from
the reader's side.

## The read path — non-destructive, merged

A `Reader` keeps **one cursor per lane**. `read()`:

1. peeks each lane's next descriptor, accepting it only when its `lane_sequence`
   equals the cursor's position (anything else is mid-write or not-yet-written),
2. yields the one with the smallest global sequence — stitching the lanes back
   into chronological order,
3. copies the bytes out of the arena and **re-validates** under a *seqlock*: an
   `Acquire` fence, then re-check the descriptor's `lane_sequence` and that the bytes
   weren't reclaimed mid-copy — if either moved, discard and re-resolve,
4. enriches it (severity + real timestamps) and returns it.

Reading removes nothing, so signals are retained with no reader attached, and
every reader sees every signal still retained. A reader that fell behind a
lane's `tail` silently skips ahead to the oldest retained signal — eviction is
not a per-reader event to report but a property of the lane. A signal whose
bytes won't parse (possible only through the
[pathological-pressure caveat](#lineage-and-caveats)) is skipped the same way,
with a WARN logged, rather than failing the subscription.

Two reading modes. Awaiting `read()` parks when nothing is yieldable — caught up,
or the only candidate is mid-write — by registering its waker against a version
counter the next `write` bumps; no timer, no spin, no cap on how many readers
park at once, and `read()` cannot fail. `try_read()` is the non-parking partner:
the next signal if one is immediately available, `None` otherwise — use it to
drain the retained signals without awaiting (a panic-dump, a one-shot snapshot).

One honesty note on step 2: the merge is chronological for signals whose writes
do not race. A writer stalled between claiming its global sequence and
publishing can make a reader yield a later sequence first — the merge is
best-effort under concurrent writers, not a total order.

## The ingest side: BufferLayer

On a node, signals reach the buffer through **`BufferLayer`**
(`src/tracing/layer.rs`, always compiled). It is a standalone `tracing`
**subscriber**, and it is **no_std** — it tracks live spans' fields and
parentage in its own map instead of the std-only subscriber registry, so it
needs no `tracing-subscriber` `Registry`. Building the logs program's `Launcher`
installs it as the program's global subscriber (a no-op if one already exists,
e.g. under a test harness). The **`console`** cargo feature adds the std
machinery to also print each event to stdout at INFO and above; without it the
same subscriber captures into the buffer but prints nothing. For each event the layer builds
a `log_record`: the message becomes the OTLP `body`; the event's **own** fields
become `attributes`; and the record carries the innermost enclosing span's id
and the task's trace id. `task_id`, `pid`, `program_name` and the rest of the
span convention are not copied onto the record — they reach the buffer on the
span signals themselves, and a log links to them through those ids.
Events at levels routed to no lane return before serializing anything.

The layer is **lock-free into the buffer from any thread**: each event packs
through its own short-lived `Writer` (one small allocation per event); span
bookkeeping (create/record/close, and reading the innermost entered span's ids
for an event — plus, on `console` builds, a pointer-clone of the scope for the
printed line) takes a brief critical section. One cost to know about: composing
the layer enables every routed callsite — with the default config that means
`trace!` node-wide, captured into the ring but still filtered out of the
console.

## The viewer: `logs` in the shell

`logs` is an ordinary shell command. How to *use* it — the viewer keys, `logs
dump`, `logs stream <url>`, the modes, `-l` — lives on the
[user-facing Logs page](../features/logs.md); this section is how the node side
and the client stream are wired underneath.

The node side has two output shapes, chosen by the `dump` bit of
`LogsArgs.Data.flags`. Without it, the node drives a client-hosted
`LogsArgs.Stream` (`view` and `stream`); with it, the node emits the signals as
Dusk values on the command's own output stream (`dump`). The other two flag bits,
`replay` and `follow`, pick the `Reader`'s start and whether it tails. The
callback path is described next; the values path follows.

On the callback path the client hosts two capabilities, and the split is what
keeps a destination from being opened before the command runs. `LogsArgs.Server`
— the args' `server` half — has one method, `openStream()`, and it is a builder:
it holds *how* to make the stream, not a made one. The node calls it from the
`logs` process's `main`, before the process signals ready. Everything the client
opens — the pager's alternate screen, a file, a collector connection — is opened
inside that call, so building a command's args opens nothing: `hi () { logs }`
defines a function, and the shell compiles the word `logs` into `ProgramArgs`
through the client callback right then, long before `hi` is ever run.

What `openStream` returns is the stream itself, and there the node side is
deliberately dumb. `LogsArgs.Stream` has two methods —
`send(entries)` (capnp-streaming) and `stop()`, a long-poll mirroring the shell's
`ShStop`. The node-side process mints a replay-then-follow `Reader`, streams
batched signals into `send` never knowing what receives them, and holds one
pending `stop()` call. A batch is whatever the buffer holds when a send is
possible: awaiting `send` is the flow-control gate (capnp streaming credit), and
while it withholds, the ring gathers the next batch — batch sizes follow the
client's absorption rate, no timers in the path. The streaming is `main`'s own
work, so when the client answers the long-poll `main` returns and the process
exits on its own; `output()`, which until then only held the command's output
stream open, finishes it, and the shell reaps the process it finds already gone.
A failed `send` keeps its batch and retries, paced by the round-trip; only the
connection dying ends the stream early.

Which `LogsArgs.Stream` the node talks to is the client's choice — the viewer, or
one of the streaming sinks. Every sink consumes the same `send` and converts the
capnp wire form — which mirrors the OTLP proto field-for-field — once into its
output: OTLP/JSON for `file://` and `http(s)://`, OTLP/gRPC for `otlp://`. The
viewer instead renders each batch into styled pager lines — dim timestamp,
colored level, message, cyan `key=value` fields (the event's own), wrapped
to the terminal width with continuation rows hanging past the timestamp and level
columns. The network sinks retry a failed connection indefinitely, paced by the
send round-trip rather than a timer, and awaiting that retry is itself the
backpressure — a down collector parks the node the way a paused viewer does.

The values path (`dump`, the `dump` flag bit set) leaves `LogsArgs.Server`
unused and never reaches a `LogsArgs.Stream` — `main` never calls `openStream`,
and the builder a `dump` command carries fails if it ever is
called. The same node-side `Reader` drains the buffer, but each signal is
converted into a `Value::Record` (tagged with `signalTypeId`, its attributes a
nested record tagged with `attributesTypeId`) and sent on the
output stream the shell already handed every program — the channel `ps` and the
rest write to — so a non-interactive caller receives the entries as its command
result. `--replay-only` returns once the replay history is drained (the stream's
`done`); a follow runs until the shell stops the command, which drops the
`output()` call and kills the process, exactly as a `file://` follow is stopped.
Backpressure is the output stream's own send credit. The conversion lives
node-side (`dump.rs`) because, unlike the viewer and network sinks, there is no
client half to render on.

The viewer feeds its rendered lines through a small bounded channel, and that
channel is the backpressure: the `send` callback resolves only when the channel
has room, so a paused pager fills it, the unresolved call fills the stream's
flow-control window, and the node parks. NORMAL mode is exactly "stop draining
the channel"; FOLLOW resumes it. The pager runs as its own task, in raw mode (so
Ctrl-C is an ordinary key — no SIGINT). A dead connection both closes the channel
and fires a drop signal; either ends the pager task, because it can't follow a
gone node, and staying in the alternate screen would collide with the prompt that
resumes the instant `output()` returns. So on disconnect the pager saves the
collected logs to a file (the user-facing `:w`), logs the path, and restores the
terminal before returning.

## Timestamps

The layer stamps every signal's `time_unix_nano` as it assembles the record
(`tracing/convert.rs`, just before `Writer::write` appends it) with embassy's
**monotonic** clock (milliseconds since node start) — cheap, immune to
wall-clock changes, and never the caller's job. The wall-clock offset is sampled
**once** when `SignalBuffer::reader` mints
a reader (one `Dusk.time` RPC) and applied during enrichment, so the hot read
path needs no further RPCs; `reader_with_offset` takes the offset directly for
contexts with no RPC session. `time` reflects when the signal was logged;
`observed_time`, when it was read. The once-sampled offset is **frozen** for the
reader's lifetime — a `Dusk.settime` issued afterwards skews everything that
reader subsequently enriches, so re-mint readers after setting the clock.

## Lineage and caveats

The design is the in-memory log ring that real systems converge on — Linux's
`printk` ringbuffer (the descriptor-ring-over-byte-arena shape is taken straight
from it), the LMAX Disruptor (the published-position seqlock guard), `perf`/
`ftrace`'s per-CPU rings: a bounded, drop-oldest ring read non-destructively by
sequence, with the write surface sharded (here, by level) and reunified on
read.

Two honest edges:

- It is hand-written lock-free code with explicit memory orderings. The three
  ordering-critical protocols are the descriptor seqlock, the arena reclamation
  gate, and the descriptor-field gate (the fence that keeps a lapped reader from
  trusting a half-rewritten descriptor); the arena is plain atomics over two
  owned allocations, so there is no third-party reclamation and nothing to leak.
- Like `printk` itself, it is best-effort under *pathological* pressure: if a
  producer were preempted mid-write for long enough that the arena wrapped all the
  way around (thousands of signals on a real-size lane), it could overwrite a newer
  signal's bytes. This is sound (the arena is atomic — never a data race) and
  astronomically unlikely; under any normal scheduling it cannot happen. A reader
  that meets such bytes skips them and keeps going; the
  unpack is also capped (8 MiB) so corrupt bytes can't demand a giant allocation,
  and the timestamp arithmetic saturates rather than overflowing on garbage.
