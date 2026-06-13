# Tracing and Spans

Dusk runs on a **single-threaded [Embassy](https://embassy.dev) executor**: every
task on a node — sessions, processes, `init` — is polled cooperatively on one
thread. That shapes how `tracing` is used here. The headline rule:

> **Every Embassy task opens a span, and that span carries the task's `task_id`.**

A log record is only useful if you can tell which task produced it, and on a
single executor the usual "current span" machinery can't tell tasks apart on its
own (see [Why the scope is per task](#why-the-scope-is-per-task)). The `task_id`
on each task's span is what makes a record attributable.

## Opening a task's span

The id a span needs is the Embassy **task id**, and you only learn it *after* the
task's spawn token exists — but the task body needs it to build its span. The
pattern threads it through a shared `Rc<Cell<u32>>`:

```rust
#[embassy_executor::task(pool_size = 16)]
async fn process_task(task_id: Rc<Cell<u32>>, process: Box<dyn Process>, /* … */) {
    let span = info_span!(
        "process",
        task_id = task_id.get(),          // read the id stored just before spawn
        pid = process.pid(),
        program_id = process.program_id(),
        program_name = process.name(),
        namespace_id = process.namespace().id,
    );
    process.bootstrap(/* … */).instrument(span.clone()).await // run the body in the span
}

// at the spawn site:
let task_id = Rc::new(Cell::new(0));
let spawn_token = process_task(task_id.clone(), process, /* … */)?;
task_id.set(spawn_token.id());            // get the task id, store it for the body
namespace.spawner.spawn(spawn_token);
```

Step by step, whenever you spawn a task:

1. Make a `task_id: Rc<Cell<u32>>` seeded with `0` and hand a clone to the task.
2. Build the spawn token.
3. **Get the id** — `spawn_token.id()` — and `set` it into the cell.
4. Spawn the token.
5. **Inside the task, open a span** whose first field is `task_id = task_id.get()`
   (now the real id), then the task's domain fields.

If you spawn a task and don't open such a span, its log records have no `task_id`
and are mis-attributed — so the span is not optional.

## Instrument, don't hold a guard

Run the task body with **`.instrument(span)`**, not `let _g = span.enter()` held
across an `.await`. `Instrument` enters the span before each poll and exits it
after, so the span is "current" only while *this* task is being polled. A guard
held across `.await` stays entered while the executor polls **other** tasks,
leaking this task's span into theirs.

For synchronous logging outside the instrumented future — a crash or an exit
line — wrap it in `span.in_scope(|| …)` so it still gets the task's span:

```rust
span.in_scope(|| info!("init task exiting"));
```

## Why the scope is per task

When an event fires, the capture subscriber records its **scope**: the fields of
the spans enclosing it (so a `process bootstrap` event picks up the `pid`,
`program_id`, … from its `process` span). The subscriber that does this,
[`BufferLayer`](logs.md), is the subscriber itself — there is no std-only
`tracing` registry underneath, so it works the same under `no_std` on a node.

On the single executor the entered-span stack is shared by every task on the
thread, so `BufferLayer` does **not** trust raw nesting. It tags each entered
span with the `task_id` of the task it belongs to (taken from that task's root
span, inherited by child spans), and an event's scope is only the **current
task's** spans. So even if a span is somehow left entered across another task's
`.await`, it cannot widen an unrelated task's scope — the `task_id` keeps each
task's stack its own.

before any task exists; it falls through to **`task_id` 0**.

## Field conventions

- `task_id` is **always the first field** on a task span.
- Domain fields follow: `namespace_id`, `pid`, `program_id`, `program_name`,
  `program_version` — whatever the task is about.
- The node-assigned u64 ids (`pid`, `program_id`, `namespace_id`, `task_id`) are
  rendered as **bare hex** everywhere — the console and every streamed record —
  so one id reads the same in the logs as in your log store, and a JSON store
  can match it exactly. (The buffer's enrichment does this on the way out; see
  the [log buffer](logs.md).)

## Executor instrumentation: the `tracing` feature

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
reaches the log buffer's writer, whose timestamping calls `Instant::now()` and
takes that same mutex, and the node deadlocks. Wakers stay cheap and lock-free;
nothing traces from wake context.

These hooks emit **events** — point-in-time markers captured by
[`BufferLayer`](logs.md) like any other event and exported as OTLP **logs**. They
are not OTLP *trace* spans with a duration: dusk has no trace-signal exporter, and
`LogRecord.trace_id` / `span_id` are never populated. An APM backend that draws
its timeline from OTLP **traces** therefore won't render these as task spans
without a trace exporter, which dusk does not yet ship.

## Where it lives

- **The convention** — opening a span per task — is at each task function (e.g.
  `dusk_core`'s `init_task` / `process_task`, `sh`'s `sh_exec`).
- **The executor hooks** — the always-on and `tracing`-gated `_embassy_trace_*`
  functions — are in `dusk/src/dusk_core/src/trace.rs`.
- **The capture and per-task scoping** is in `base/logs/src/layer/`: `BufferLayer`
  (the subscriber, span table, and the `(span_id, task_id)` entered stack) plus
  the `collect` → `record` → `console` pipeline that turns an event into a packed
  log record.
