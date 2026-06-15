# Tracing and Spans

Dusk runs on a **single-threaded [Embassy](https://embassy.dev) executor** — every
task on a node (sessions, processes, `init`, and whatever a program spawns) is
polled cooperatively on one thread. A log is only useful if you can tell which
task produced it and where it ran, and on one shared executor the usual "current
span" nesting can't tell tasks apart on its own. So Dusk puts the burden on a
small contract you follow **every time you spawn a task**.

That contract — the golden rules below — is the part of this page that matters.
Everything after it is how the rules work behind the scenes.

## The golden rules

Every task you spawn opens **one span**, its *root span*, and that span is what
ties every log under the task back to where it ran. Four rules govern it:

1. **Mark the root span a task root** with `__new_task_id__`, set to the embassy
   task id. The deliberately ugly name is a flag to the capture layer: "this span
   begins a new task." Without it, the task's logs are not attributable.
2. **Carry `namespace_id`.** A task spawned *anywhere* — not only inside a
   process — must carry `namespace_id` on its root span, so every log under it
   links to the namespace it ran in.
3. **Also carry `pid` when a process spawned the task.** Then every log under it
   links to the process that created it, too.
4. **Run the body with `.instrument(span)` — never a guard held across `.await`.**
   `Instrument` enters the span before each poll and exits after, so it is
   "current" only while *this* task is polled. A `let _guard = span.enter()` held
   across an `.await` stays entered while the executor polls *other* tasks, leaking
   this span into theirs.

A root span carries the marker and these fields; **child spans inherit them and
must not repeat them**, and a span that is *not* a task root (`logs_stream`, a span
over a future inside an RPC handler) neither marks itself nor restates them. The
node's own task spans already follow the rules (`init`, `session`, `process`); a
program that spawns work must too — `sh`'s `sh_exec` task carries `__new_task_id__`,
`pid`, and `namespace_id`.

### Opening the span

The embassy task id isn't known until the spawn token exists, but the task body
needs it to build the span. Thread it through a shared `Rc<Cell<u32>>`:

```rust
#[embassy_executor::task(pool_size = 16)]
async fn process_task(task_id: Rc<Cell<u32>>, process: Box<dyn Process>, /* … */) {
    let span = info_span!(
        "process",
        __new_task_id__ = task_id.get(),  // rule 1: mark the task root (value is the embassy task id)
        pid = process.pid(),              // rule 3
        program_id = process.program_id(),
        program_name = process.name(),
        namespace_id = process.namespace().id,   // rule 2
    );
    process.bootstrap(/* … */).instrument(span).await   // rule 4: instrument, don't guard
}

// at the spawn site:
let task_id = Rc::new(Cell::new(0));
let spawn_token = process_task(task_id.clone(), process, /* … */)?;
task_id.set(spawn_token.id());            // the id is known only now; store it for the body
namespace.spawner.spawn(spawn_token);
```

For synchronous logging *outside* the instrumented future — a crash line, an exit
line — wrap it in `span.in_scope(|| …)` so it still lands in the task's span:

```rust
span.in_scope(|| info!("init task exiting"));
```

### Field conventions

- `__new_task_id__` is **always the first field** on a task root span. It surfaces
  on every log as the `task_id` attribute (see
  [behind the scenes](#behind-the-scenes-telling-tasks-apart)).
- Domain fields follow: `namespace_id`, `pid`, `program_id`, `program_name`,
  `program_version` — whatever the task is about.
- The node-assigned u64 ids (`pid`, `program_id`, `namespace_id`, `task_id`) render
  as **bare hex** everywhere — the console and every streamed signal — so one id
  reads the same in the logs as in your log store, and a JSON store can match it
  exactly. (The buffer's enrichment does this on the way out; see the
  [signal buffer](logs.md).)

## Behind the scenes: telling tasks apart

The capture subscriber, [`BufferLayer`](logs.md), *is* the subscriber — not a layer
over a std-only `tracing` registry — so it owns its own span table and entered-span
stack and works the same under `no_std` on a node. When an event fires it records
the event's **scope**: the fields of the spans the event is inside, so a `process
bootstrap` event inherits the `pid` and `program_id` from its `process` span.

The catch is that there is **one** entered-span stack, shared by every task on the
single executor. Raw nesting can't be trusted: a span left entered across an
`.await` (the guard that [rule 4](#the-golden-rules) forbids) would sit on the
stack while another task is polled and leak into it. So the layer has to know which
entered spans belong to the task running *now*.

The obvious key — the embassy task id — doesn't work, and this is the trap the
rules step around. **The embassy task id is reused.** It is a pointer into a
fixed-size task pool slot; when a task finishes, its slot is reclaimed, and the
next task spawned can claim the same slot and be handed the *same* id. So the
embassy id names a slot at a moment, not a task over time — two unrelated tasks can
wear it one after another.

The layer sidesteps that entirely:

- **Every span gets its own id** from a monotonic counter that never repeats for the
  life of the node.
- **A task is identified by its root span's id.** A span declares itself a root by
  the *presence* of `__new_task_id__` — the layer keys on that the field is there,
  and ignores its value (the reused embassy id). The root span's own, never-reused
  id becomes the task's identity.
- **The marker is rewritten to `task_id`** on the way in, so the embassy id still
  rides on every log as an ordinary attribute — kept as data, never used as
  identity. Only a span carrying `__new_task_id__` is a task root; a span that
  merely mentions `task_id` is not.

Each entry on the entered stack is then tagged with the id of the task root it
belongs to: a root tags itself, a child inherits the enclosing root's tag. An
event's scope is only the entries sharing the current task's tag — so a span leaked
across another task's `.await` cannot widen an unrelated task's scope. A span
entered before any task root tags as id 0.

That same identity is the trace id: **a task is a trace, and `trace_id` is its root
span's id.** A log's trace is its task, a span's trace is the task it belongs to,
and a task's nested spans all share it — distinct forever, where the embassy id
would have collided.

## Executor instrumentatios;
use capnp::serialize_packed;n: the `tracing` feature

The spans above are opened by *task code*. The executor itself also reports its
scheduling, through link-time hooks Embassy calls at each task transition — the
`_embassy_trace_*` functions in `dusk_core/src/trace.rs`. They surface task churn
no application span sees:

- **Always emitted** — `task new` and `task end`: one event as a task is spawned,
  one as it finishes, each carrying `executor_id` and `task_id`.
- **Behind the `tracing` feature** — the high-frequency hooks: `executor poll`,
  `task exec begin`, `task exec end`, and `executor idle`. They fire on every
  poll and every idle, so they are off by default; build `dusk_core` with
  `--features tracing` to opt in to per-poll detail.

One hook stays deliberately silent. `_embassy_trace_task_ready_begin` runs inside
`wake()`, and a wake can fire from a foreign context — the embassy-time alarm
thread wakes a task while holding its own mutex. A `tracing` event from there
reaches the signal buffer's writer, whose timestamping calls `Instant::now()` and
takes that same mutex, and the node deadlocks. Wakers stay cheap and lock-free;
nothing traces from wake context.

These hooks emit **events** — point-in-time markers captured by
[`BufferLayer`](logs.md) like any other event and exported as OTLP **logs**,
distinct from the durational task spans that become OTLP traces (next).

## Spans as OTLP traces

A task's spans don't only scope logs — when their lane is routed, each span's
**close** is captured as a `Signal` span: a real OTLP span carrying its id, parent,
name, kind, start/end, and its deduplicated scope as attributes, with the task root
id as its `trace_id`. A span rides its **own** level's lane (`info_span!` → info,
`debug_span!` → debug), so it is kept exactly when that level is. Nothing is written
when a span opens — OTLP can't represent an unfinished span, so only the complete
close signal is emitted.

The mapping to a trace is fixed by the node's structure:

- **`trace_id` is the task root span's id** (above) — the namespace travels as the
  `namespace_id` *attribute* (rule 2), not as the trace id.
- **A service per program.** `service.name` is the span's `program_name`; spans with
  no program (`init`, client sessions, core internals) fall to the `core` service.

Spans are streamed like any other signal, and each sink renders them its own way:
the `file://` and `http(s)://` sinks emit each span as OTLP/JSON (with its
`info_span!`/`debug_span!` level carried as a `severityNumber`), the `otlp://`
exporter reshapes each onto the OTLP **trace** signal field-for-field (grouped one
`ResourceSpans` per service), and the interactive `logs view` omits them.

## Where it lives

- **The convention** — opening a span per task — is at each task function (e.g.
  `dusk_core`'s `init_task` / `process_task`, `sh`'s `sh_exec`).
- **The executor hooks** — the always-on and `tracing`-gated `_embassy_trace_*`
  functions — are in `dusk/src/dusk_core/src/trace.rs`.
- **The capture and per-task scoping** is in `base/logs/src/layer/`: `BufferLayer`
  (the subscriber, span table, and the `(span_id, task root span id)` entered stack)
  plus the `collect` → `record` → `console` pipeline that turns an event into a
  packed log record. The same layer writes each span's close signal.
- **The OTLP trace reshaping** — mapping each span to a `Span` and grouping them by
  service — is in `base/logs/src/client/stream/otlp/`, the `otlp://` stream only.
