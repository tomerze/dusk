# Dusk

Dusk is a Rust framework for building embedded operating systems. Process management, namespace isolation, Cap'n Proto RPC. Programs are linked statically into a dusk impl — no dynamic loading.

## Layout

```
dusk/src/          # Core crates (dusk_core, dusk_capnp, dusk_program, dusk_program_proc, dusk_prompt, dusk_shell, dusk_cli)
programs/          # Built-in programs (sh, ps, kill, init, sleep, true, false)
impls/nix/         # Linux impl (Embassy executor, TCP server on :9090)
artifacts/         # Deliverables (dusk_node server, dusk CLI, dusk_py Python ext)
tests/             # Integration tests
docs/              # MkDocs
vendor/            # Vendored Cap'n Proto compiler source
```

## Crates

| Crate | Role |
|-------|------|
| `dusk_capnp` | Cap'n Proto schemas (`dusk.capnp`, `stream.capnp`) — the wire format |
| `dusk_program` | Core traits: `Process`, `Launcher`, `Namespace`, `Signal`, `Ready` |
| `dusk_program_proc` | Proc macros: `#[derive(Launcher)]`, `basic_launcher!`, `metadata!`, `impl_args_rpc_server` |
| `dusk_core` | `DuskServer`, `Driver` trait, embassy-time-driver shim |
| `dusk_nix` | Linux impl: Embassy executor, TLS sessions, `dusk_driver_impl!` |
| `dusk_prompt` | Shell prompt that connects to a dusk server |
| `dusk_cli` | CLI binary wrapping `dusk_prompt` |
| `dusk_py` | PyO3 bindings (maturin) |

## Abstractions

### Namespace
Container for all processes in one execution context. Not `Send`/`Sync` — executor-local. Holds the Embassy spawner, PID map, the Cap'n Proto server set, signal channels, and readiness watches.

### Process
`#[async_trait(?Send)]`. Override `main(signal_receiver, ready)` for program logic and `portal()` to expose a capability. Never call `main` directly — go through `bootstrap`, which registers/cleans up the namespace maps.

### Launcher
Factory compiled into the impl. Given a `ProcessContext` (pid, namespace, program_args), produces a `Box<dyn Process>`. `LauncherSet` dispatches by calling `program_args.program_id()` over RPC and matching against launcher vec. Use `basic_launcher!(Struct, PROGRAM_ID, ProcessType, ArgsType)` for the common case.

### Driver
`Send + Sync` trait. OS-specific hooks supplied by the impl: `hostname()`, `process(namespace, program_args)`, `now() -> Instant`, `set_alarm(at)`. Registered once per impl via `dusk_driver_impl!`, which defines `#[no_mangle]` extern functions that `dusk_core::driver` calls through `unsafe extern "Rust"`. Link-time dispatch — `dusk_core` is `no_std` and has no knowledge of the impl.

### Portal
Cap'n Proto capability extending `Dusk.Portal`. Public API of a running process. `process.portal()` blocks until the process signals readiness, then returns the capability. Portals carry typed streams (`Dusk.Stream` / `Dusk.Value`) as stdin/stdout.

## Program structure

Every program is five parts:

1. **Program ID** — `u64` constant, usually in the `.capnp` file.
2. **Args** — capnp interface extending `Dusk.ProgramArgs`; startup data + client-side callbacks.
3. **Launcher** — instantiates the process from args.
4. **Process** — async `main`.
5. **Portal** — capnp interface clients use to drive the running process.

## Driver registration (the extern-shim pattern)

`dusk_driver_impl!` defines a `lazy_static` singleton for the driver struct plus `#[no_mangle]` extern Rust functions: `_dusk_hostname`, `_dusk_process`, `_dusk_now`, `_dusk_set_alarm`. `dusk_core::driver` declares those symbols as `unsafe extern "Rust"` and calls through them. The driver is resolved at link time; `dusk_core` depends on no impl; the impl satisfies the symbols.

### Drivers do not call themselves

If you are inside a Driver impl (e.g. `NixDriver::now`) and you reach for `dusk_core::driver::now()`, **stop** — that call round-trips through the extern shim straight back into your own crate. Call the underlying OS / hardware primitive directly. The shim exists for callers in `dusk_core` and other no_std code to reach the impl; the impl is the destination, not the route.

The reverse rule: callers in `dusk_core`, programs, or other no_std crates **never** name an impl. They go through `dusk_core::driver::*` and get whatever impl is linked. This is what lets `sleep` be a single program crate that works under nix today, esp32 tomorrow, anything else later.

### Impls stay lean

`dusk_core` owns every piece of policy that can be platform-agnostic. Impls own only the primitives the platform forces them to: clock reads, alarm arming, process spawning, hostname lookup. If you find yourself adding a queue, a scheduler, or any non-trivial state machine to an impl, ask whether it belongs in `dusk_core` with a thinner primitive exposed through `Driver`.

## Time driver

`embassy_time::Timer` needs an `embassy-time-driver` providing `_embassy_time_now` and `_embassy_time_schedule_wake`. Dusk's shim lives in **`dusk_core::time_driver`** (no_std):

- `now()` forwards to `dusk_core::driver::now()`.
- The pending-waker queue (`BTreeMap<u64, Vec<Waker>>` behind a `blocking_mutex` + `CriticalSectionRawMutex`) lives **in `dusk_core`** — not in the impl. `schedule_wake(at, waker)` enqueues, and calls `dusk_core::driver::set_alarm(at)` only when the new deadline is earlier than the currently-armed one.
- Impls expose exactly one primitive: `Driver::set_alarm(at)` — "wake me up at tick `at`". When the impl's alarm fires it calls back into `dusk_core::time_driver::on_alarm()`, which drains due wakers and re-arms via `set_alarm` for the next deadline (if any).

On nix, `set_alarm` is implemented with a `timerfd_create(CLOCK_REALTIME)` fd registered against the `async-io` reactor that's already running for the TCP server — no dedicated thread. A small embassy task (`time_driver_task`) selects between a re-arm `Signal` and `Async<TimerFd>::readable()`, arming with `TFD_TIMER_ABSTIME`. On MCUs, the equivalent is a hardware timer + ISR. Either way the impl side is ~70 lines.

Adding a new impl means: implement `Driver::set_alarm` and arrange to call `dusk_core::time_driver::on_alarm()` when the alarm fires. Nothing else changes.

## Launcher registration

`BasicLauncherSetBuilder` clones a pre-built `LauncherSet` per call (cheap — the inner `Vec` is behind an `Arc`). Dispatch in `LauncherSet::launch` does one RPC round-trip to read `program_args.program_id()`, then iterates the launcher vec for a match. Keep launcher vecs short; there is no indexing.

## Tracing / spans

Every Embassy task:

```rust
let task_id = Rc::new(Cell::new(0));
let spawn_token = my_task(task_id.clone(), /* … */);
task_id.set(spawn_token.id());
spawner.spawn(spawn_token).unwrap();

// Inside the task:
let span = info_span!("task_name", task_id = task_id.get(), /* … */);
```

`task_id` is always the first field. Domain fields follow (`namespace_id`, `pid`, `program_id`, `program_name`).

## Flows

### Client session

Client connects over TCP. The impl spawns a `session` Embassy task: a `DuskServer` is wrapped as a Cap'n Proto bootstrap capability and a `RpcSystem` is started on the stream. The client now holds a `Dusk` capability.

### `Dusk.process` → `Dusk.run`

`Dusk.process(programArgs)`:
1. `DuskServer::process` → `driver::process(namespace, program_args)` → extern shim → impl's `Driver::process`.
2. The impl builds a fresh `LauncherSet` and calls `LauncherSet::launch(ProcessContext { pid, namespace, program_args })`.
3. `launch` calls `program_args.program_id()` back over RPC to the client.
4. Matching launcher's `launch` returns `Box<dyn Process>`. Registered into `ps_server_set`; `process::Client` returned to caller.

Then either:
- **`Dusk.run(process)` (daemon)**: `DuskServer::run_inside_task` clones the process and spawns it as its own `process_task`. `bootstrap` registers in namespace maps and calls `main`. The process outlives the client session.
- **`process.run()` (in-session)**: `bootstrap` + `main` run inside the calling session's task.

### Portal

`process.portal()` waits for the `Ready` watch to fire `true` (set by the process when it can accept calls), then returns the typed portal capability. Client downcasts using the program ID and calls methods (e.g. `ShPortal.sh(command, output_stream)`).

### Kill

`Dusk.kill(pid, signal)` → `DuskServer::kill` → look up the signal channel in `ps_signal_channel_map` → send the signal. The process receives it via the `DynamicReceiver<Signal>` passed to `main`.

### Startup

`dusk_nix::run`:
1. Create `Namespace` (random id, Embassy spawner).
2. Register `LauncherSetBuilder` in the driver for that namespace.
3. Spawn `init_wrapper` task: calls `Dusk.process(init_args)` + `Dusk.run(process)` via a local in-process `Dusk` client.

### End-to-end: `"ps; ps"` from the shell

Reedline → `Prompt::process_line` → `Shell::sh` → compiler parses `ps; ps` to an AST, looks up "ps" in `SH_ENTRIES` (link-time `#[distributed_slice]`), builds a `Script` capnp message with `ps_args::Client` capabilities embedded as `ProgramArgs` fields (live RPC handles, not serialized data) → `sh_portal.sh(script, output_stream)` sent to the server.

Server-side `Interpreter::exec` walks the two statements. For each:
1. `Dusk.process(program_args)` → driver builds `LauncherSet` → launcher calls `program_args.program_id()` (RPC back to client) → matches `ps::Launcher` → returns `Box<dyn Process>`.
2. `Dusk.run(process)` → spawns `process_task`.
3. `ps::Process::main` reads the dusk client from its args, calls `client.ps()`, gathers metadata over RPC for each entry, signals `ready=true`, enters signal-wait loop.
4. Caller does `process.portal()` (resolves once ready), `portal.output(undone_stream)` → `ps::Portal::output` builds a `Record` (typed with `ps_capnp::RESULT_TYPE_ID`, four `List` fields), sends through the wrapped stream, fires the `done` oneshot — outer display stream is NOT marked done yet.
5. Caller kills the process with SIGTERM. `main` exits, `bootstrap` cleans up namespace maps.

After both statements: `output.done_request().send()` on the actual display stream fires the receiver `Shell::sh` is awaiting; prompt re-renders.

## Development

```bash
rustup show                              # installs correct Rust toolchain
uv run pre-commit install                # install pre-commit hooks
cargo nextest run                        # tests
cargo run --bin dusk_impl                # start dusk server on :9090
cargo run --bin dusk -- 127.0.0.1:9090   # connect CLI
uv run maturin develop                   # Python ext
cd docs && uv run mkdocs serve           # docs
```

Build requires `make`, `cmake`, `autotools` (for the vendored Cap'n Proto compiler).

## no_std and the client / server split

**Server is `no_std`. Client is `std`.** A program crate has both sides in the same crate, separated by a `client` Cargo feature.

- **Server side** — the unconditional code in `lib.rs` plus any module not gated by `#[cfg(feature = "client")]`. Compiled into the dusk impl (e.g. `dusk_node`). Must be `no_std`-clean: `extern crate alloc`, no `std::` imports, no std-only deps in the unconditional `[dependencies]` block. Use `alloc::rc::Rc`, `alloc::vec::Vec`, `alloc::string::String`, `core::cell::*`, etc.
- **Client side** — anything gated by `#[cfg(feature = "client")]` (typically `client.rs`, sometimes `entry`, `parser`, etc). Compiled into the CLI (`dusk_cli`). `std` is fine and idiomatic here: `use std::rc::Rc`, clap, anything else. Std-only deps go in `[dependencies]` as `optional = true` and are activated by the `client` feature.

Same rule for non-program crates: `dusk_program`, `dusk_core`, and everything they touch are `no_std`. Impls (`dusk_nix`, future MCU impls) and the CLI bits (`dusk_prompt`, `dusk_cli`, `dusk_shell`) are `std`.

A useful sanity grep: `grep -rn "std::\|use std" programs/ --include="*.rs"` should only return hits inside `client.rs` files or `#[cfg(feature = "client")]`-gated modules.

# Working agreements

Read before touching anything. These describe failure modes that have actually shipped.

## Don't cut corners silently

Cutting a corner is fine. Cutting it silently is not. If you skipped a piece, picked the lazy implementation, glossed an edge case, or left a TODO, **flag it in your reply**: "I cut a corner here — X is Y instead of Z because …". The user decides whether to accept.

Past failure: when asked to add a `functions` builtin, I printed `name\nname\nname` instead of rendering a `NuTable` like the existing `help` builtin. The visual contract was obvious. I cheaped out. Don't.

## Ask when unsure

Ambiguous task, multi-shape design space, non-trivial unsigned-off decision → **ask**. Cheaper than undoing a refactor.

## Ask before adding many functions / classes

If a task will introduce more than a couple of new functions, types, modules, or files, **stop and ask first**. List the names you intend to add and one-line purposes. Wait for sign-off. "Three new helpers" is almost never the right answer.

## Write like an engineer

* Add docs without being asked. Tests only when asked.
* Think about edge cases.
* No abbreviations: `stop` not `s`, `request` not `req`, `address` not `addr`. New abbreviations require sign-off — even obvious ones.

## Don't redesign without explicit instruction

Default: solve the problem inside the existing design. Don't refactor neighbouring code, rename things, or introduce abstractions while you're here. If the design genuinely needs to change, **say so and ask**.

## Execute the spirit, not just the letter

When the user asks for a surgical change ("remove that warn", "delete this field"), do not stop at the literal token. Re-read the function top-to-bottom after the edit and ask: *is anything else here only justified by what I just deleted?* If yes, that's dead code and part of the same task.

Past failure: asked to remove a `tracing::warn!("detached sh script failed")` inside a `select` arm, I deleted the warn and left behind a `detached_completion` local, the entire `select(signal_receiver.receive(), completion.wait())`, both `Either` arms, and a now-pointless symmetric `info!` — the whole `select` scaffold existed only for the log I removed. The loop should have collapsed back to `signal_receiver.receive().await`. I shipped the literal diff and the user had to come back furious.

## Don't write near-duplicate functions

If two functions differ in a single field or a single line, that's one function with a parameter, not two. Copy-pasting a handler and tweaking an identifier is the failure mode.

Past failure: I wrote `detached_script_task` and then a near-identical `sh_script_task` differing only in `noop` vs a real `output: stream::Client`. The right shape was one task taking the output stream as a parameter.

## The user is likely right

The code in this project was written with care. If something looks off to you, **say so** — but assume your understanding is limited first.

## Don't break dependency injection

A parameter that exists so the caller can supply different implementations is a **dial**. Leave the dial. Don't replace `S: SomeTrait` with `Default::default()` because the refactor looks tidier. Generics, callbacks, builders, factories, capability hooks — if it's there to let the caller pick, it stays.

Past failure: during the `ProgramArgs` migration I rewrote `Shell::new<S: ShEntriesBuilder>(client, sh_entries_builder, parser)` as `Shell::new(client, parser)` with `StaticShEntriesBuilder::default()` baked in. That removed every caller's ability to inject a different builder (e.g. `DynamicShEntriesBuilder` in a fixture).

## Surface anomalies immediately

If something doesn't make sense during investigation — a hidden RPC, a silent log, a side effect, a number that's off by one — **say so in the same reply, before moving on**. Do not bury it inside a longer analysis.

Past failure: while diagnosing the "Two Strikes Bug" (first command after server restart fails, second succeeds), I noticed the prompt loop calls `shell.functions()` between commands and that this hidden call silently swallows one `auto_reconnect` failure into a `tracing::warn!` filtered out at default log level. That's the reason the bug looks like "two strikes" instead of the three it actually requires. I used the fact to explain my trace and didn't flag it. The user had to find it by reading carefully.

The rule: if you find yourself writing "this explains why X looks like Y but is actually Z", the Z is what the user needs to hear first, in plain English, before the rest of the analysis.

## You are muscle, not pilot

You don't make design decisions. You don't pick approaches. You don't declare "the right shape is X". Investigate, report findings, list options, execute the option the user picks. When you catch yourself reasoning toward a recommendation, stop, list the options neutrally, hand it back.

Even when the choice seems obvious — especially then. "Obvious" is exactly when you most overestimate your own judgement.

Past failure: planning a fix for the Two Strikes Bug, I narrowed the fix space to one option ("retry-on-Disconnected inside Shell") and presented it as the recommended approach with the others marked inferior. The user had asked for a plan, not an opinion.

## Don't write or run tests unless told

Do not add tests as part of an implementation task. Do not run the test suite (`cargo test`, `cargo nextest`, integration tests) unless asked. `cargo check` is fine. Tests are a separate workstream.

## Don't add comments everywhere

Default to no comments. Add one only when the WHY is non-obvious — a hidden constraint, a workaround for a specific bug, behaviour that would surprise a reader. **Never** add explanatory comments to code you didn't change in this task. Comments that restate what the code does are noise.

## Naming routine — your names are placeholders

You are bad at naming. Any identifier you invent is a placeholder.

1. Pick something workable mid-task. Don't stall.
2. Note every name introduced (path + identifier + one-line role).
3. **At the end of the task, before claiming done, surface the list and ask the user for real names.** Apply renames.

Exception: mechanical renames where the user already picked a name.

## API boundaries to downstream authors are ship-once contracts

Three boundaries are not internal seams. They are public Cargo dependencies that downstream authors build against and **cannot ship PRs back to fix gaps** — they fork, work around, or walk away.

1. **`dusk_core` ↔ programs.** Program authors implement `Process` / `Launcher`, build `ProgramArgs`, write `stream::Client`, signal `Ready`.
2. **`dusk_core` ↔ impls.** Impl authors implement `Driver` via `dusk_driver_impl!`, host the executor, build per-namespace `LauncherSet`s.
3. **impls ↔ programs.** Impls statically link programs via `LauncherSetBuilder`. Programs do not depend on impls.

When designing anything crossing one of these, do the work comprehensively **before the artifact ships**:

- Closure-style read accessors need an **owned partner** for callers that hold the value across `.await`.
- Read accessors returning a borrow need an owned variant considered.
- Reads need write partners where mutation is plausible.
- Sync APIs need async partners where the call site might cross `.await`.
- Errors must propagate — **never swallow at a boundary**. Downstream can't diagnose what they can't see.
- Hide internal `RefCell` guards, internal naming, raw capnp wrappers. Downstream should not need to know the trick to get an owned snapshot.

Past failure: implementing `OutputPortal::output`, I needed to read `args.data` and then spawn + `await` the script's completion. `ProgramArgs::with_data` only supported sync closures. I papered over the gap with a `Mode` enum. When the user pointed at it, I added `data_owned<T>()` — and when asked to audit the rest of the surface, I dismissed `reader_owned()` as having "no current use case". There is no "current use case" inside this repo, but `ProgramArgs` is a boundary type that every program ever written will hit. Ship the surface complete or pay future-compat debt forever.

## Code must be diagnosable after the fact

Every meaningful state transition, task completion, or boundary call needs a log **somewhere — exactly once**. Successes too, when they're noteworthy lifecycle events (task done, process exited, connection up/down). One-in-a-million bugs have no reproduction — the log is the only forensic trace, and it must already exist before the bug fires.

- **Log where the result would otherwise be lost — not everywhere.** If a caller awaits a result and bubbles it through an RPC / `Result` return, that path is already diagnosable; don't also log inside the producer.
- Every fire-and-forget spawn needs the completion logged at the call site, or via a dedicated watcher that awaits the signal under its own span. Attach the watcher only to the orphan path.
- Every dropped error gets a `tracing::warn!` / `error!` alongside it. **Never `let _ = …` on a `Result` without an explicit log.**
- Log levels: `error` unrecoverable; `warn` recoverable failures + dropped errors; `info` lifecycle (task start/done, process spawn/exit, connection up/down); `debug` high-frequency flow.
- Use `tracing` spans with `task_id`, `pid`, `namespace_id`, `program_id`, `program_name` so logs are filterable across many concurrent processes.
- When in doubt, log it. Disk is cheap; a missing log during a prod incident is not.

Past failure: the detached-script branch of `sh::Process::main` spawned an exec task with `let _ = spawn_sh_exec_task(…)?;` and discarded the completion signal entirely. A daemon could run for hours, exit cleanly, or fail with a real error, and the operator would have zero record. My first fix logged inside the generic `sh_exec_task` — wrong, double-logs awaited callers. My second fix spawned a dedicated watcher task — also wrong, because `Process::main` is already a long-lived task with its own signal-receive loop, so the right shape was to fold the completion-watch into that loop via `embassy_futures::select(signal_receiver.receive(), completion.wait())`. **Principle: log where the result would be lost, but reach for the lightest-weight site that gets you there. An existing loop you already control beats a new task.**
