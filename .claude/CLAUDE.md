# Dusk

Dusk is a Rust framework for creating embedded operating systems. It provides process management, namespace isolation, and Cap'n Proto-based RPC between processes. Programs are compiled statically into a dusk impl — no dynamic loading.

## Repository Layout

```
dusk/src/          # Core crates (dusk_core, dusk_capnp, dusk_program, dusk_program_proc, dusk_prompt, dusk_shell, dusk_cli)
programs/          # Built-in programs (sh, ps, kill, init)
impls/nix/         # Linux implementation (Embassy executor, TCP server on port 9090)
artifacts/         # Deliverable binaries (dusk_impl server, dusk CLI, dusk_py Python extension)
tests/             # Integration tests
docs/              # MkDocs documentation
vendor/            # Vendored Cap'n Proto compiler source
```

## Key Crates

| Crate | Role |
|-------|------|
| `dusk_capnp` | Cap'n Proto schema definitions (`dusk.capnp`, `stream.capnp`); the RPC wire format |
| `dusk_program` | Core traits: `Process`, `Launcher`, `Namespace`, `Signal`, `Ready` |
| `dusk_program_proc` | Proc macros: `#[derive(Launcher)]`, `basic_launcher!`, `metadata!`, `impl_args_rpc_server` |
| `dusk_core` | `DuskServer` (Cap'n Proto server impl) and `Driver` trait (OS-specific hooks) |
| `dusk_nix` | Linux impl: Embassy executor, TLS sessions, `dusk_driver_impl!` |
| `dusk_prompt` | Shell prompt connecting to a dusk server |
| `dusk_cli` | CLI binary wrapping `dusk_prompt` |
| `dusk_py` | PyO3 Python bindings built with maturin |

## Core Abstractions

### Namespace
Container for all processes in one execution context. Not `Send`/`Sync` — always executor-local. Holds the Embassy `spawner`, PID map (`ps_map`), Cap'n Proto server set (`ps_server_set`), signal channels (`ps_signal_channel_map`), and readiness watches (`ps_ready_map`).

### Process
Async trait (`#[async_trait(?Send)]`) representing a running program instance. Override `main(signal_receiver, ready)` for program logic and `portal()` to expose a Cap'n Proto capability to callers. Never call `main` directly — always go through `bootstrap`, which registers the process in the namespace maps, wires up the signal channel and ready watch, and cleans them up after `main` exits.

### Launcher
Factory compiled into the impl. Given a `ProcessContext` (pid, namespace, program_args), produces a `Box<dyn Process>`. `LauncherSet` holds all launchers and dispatches by querying `program_args.program_id()` over RPC, then calling the matching launcher. Use `basic_launcher!(Struct, PROGRAM_ID, ProcessType, ArgsType)` for the common case.

### Driver
`Send + Sync` trait providing OS-specific hooks to `dusk_core`. Methods: `hostname()`, `process(namespace, program_args)`, `now()`. Registered once per impl via `dusk_driver_impl!`, which defines `#[no_mangle]` extern functions that `dusk_core::driver` calls through `unsafe extern "Rust"` declarations — a link-time dispatch mechanism that avoids generics across the `no_std` boundary.

### Portal
A Cap'n Proto capability extending `Dusk.Portal`. This is the public API of a running process. `process.portal()` blocks until the process signals readiness, then returns the portal capability. Portals carry typed streams (`Dusk.Stream` / `Dusk.Value`) as stdin/stdout.

## Program Structure

Every program has five parts:

1. **Program ID** — `u64` constant, usually defined in the `.capnp` file.
2. **Args** — a capnp interface extending `Dusk.ProgramArgs`; carries startup data and client-side callbacks.
3. **Launcher** — instantiates the process from args.
4. **Process** — the async `main` logic.
5. **Portal** — the capnp interface clients use to interact with the running process.

## Flows

### Connecting a client session

A client connects over TCP. The impl (e.g. `dusk_nix`) accepts the connection and spawns a `session` Embassy task:

1. A `DuskServer` is created for the namespace and wrapped as a Cap'n Proto bootstrap capability.
2. A Cap'n Proto `RpcSystem` is started on the stream. The client now holds a `Dusk` capability.

### Creating and running a process (`Dusk.process` + `Dusk.run`)

When a client calls `Dusk.process(programArgs)`:

1. `DuskServer::process` calls `driver::process(namespace, program_args)`.
2. The `Driver` impl (`NixDriver`) looks up the `LauncherSetBuilder` for the namespace, builds a fresh `LauncherSet`, and calls `LauncherSet::launch(ProcessContext { pid, namespace, program_args })`.
3. `LauncherSet::launch` calls `program_args.program_id()` back over RPC to the client to learn which program to launch.
4. The matching `Launcher` is found by program ID and its `launch` method is called, returning a `Box<dyn Process>`.
5. The process is registered into `ps_server_set` and a `process::Client` capability is returned to the caller.

When the client then calls `Dusk.run(process)` (daemon mode) or `process.run()` (in-session mode):

- **Daemon (`Dusk.run`)**: `DuskServer::run_inside_task` looks up the local process server, clones the process, and spawns it as its own Embassy task via `process_task`. The task calls `process.bootstrap()`, which registers the process in namespace maps and calls `process.main()`. The process outlives the client session.
- **In-session (`process.run`)**: `bootstrap` and `main` run inside the calling session's task. The process exits when `main` returns.

### Getting a portal and interacting with a process

1. The client calls `process.portal()`.
2. On the server side, `portal()` waits for the process's `Ready` watch to fire `true` (set by the process itself when it is ready to accept calls).
3. Once ready, `ProcessMixin::portal()` is called on the concrete process type, returning its capability.
4. The client downcasts the returned `Portal` capability to the program-specific interface (e.g. `ShPortal`) using the program ID.
5. The client calls methods on the portal (e.g. `ShPortal.sh(command, output_stream)`) and reads `Dusk.Value` items from the output stream until `done()` is received.

### Killing a process (`Dusk.kill`)

1. The client calls `Dusk.kill(pid, signal)`.
2. `DuskServer::kill` looks up the signal channel for that PID in `ps_signal_channel_map` and sends the signal value.
3. The process receives it via the `DynamicReceiver<signal::Signal>` passed to `main` and handles it (e.g. exits cleanly).

### End-to-end: executing `"ps; ps"` from the shell

This traces the full path from the user pressing Enter to output being printed. The example input is `"ps; ps"` — two sequential ps invocations.

#### Phase 1 — Input reading (client side, `dusk_prompt`)

1. `Reedline` reads the line from the terminal and emits `Signal::Success("ps; ps")`.
2. `Prompt::process_line` is called. "ps" is not a builtin (`exit`, `clear`, `help`), so it falls through to the shell branch.
3. A `stream::Client` is created via the `stream_factory` closure — this is the display stream that will receive `Dusk.Value` output and render it to the terminal.
4. `Shell::sh("ps; ps", stream, done_receiver)` is called.

#### Phase 2 — Compilation (client side, `dusk_shell` + `sh` compiler)

5. `Shell::sh` calls `compiler.compile("ps; ps", script_builder)` where `script_builder` is a Cap'n Proto builder for a `sh_capnp::Script` message.
6. `parse::ast("ps; ps")` runs the nom parser. `;` separates statements. Each bare word sequence becomes a `Command`. The result is:
   ```
   Ast { statements: [
     Statement { expr: Command { program: "ps", args: [] }, background: false },
     Statement { expr: Command { program: "ps", args: [] }, background: false },
   ]}
   ```
7. For each `Command`, `compile_program_args` looks through `SH_ENTRIES` (populated at link time by `#[distributed_slice]`) for an entry whose `name` matches "ps".
8. It finds the `ps` entry and calls `PsProgramArgsBuilder::build(client, "")`, which creates a local `ps_args::Client` capability wrapping a `ps::Args { client }` server. This capability is embedded directly into the Cap'n Proto `Script` message as a `ProgramArgs` field — it is a live RPC capability, not serialized data.
9. The compiled `Script` has two statements, each carrying its own `ProgramArgs` capability handle.

#### Phase 3 — Sending the script to the sh process (client → server RPC)

10. `Shell::sh` acquires the `sh_portal` by calling `sh_process.portal_request()`. Since the sh process is already running (it was spawned when `Shell::new` was called), `portal()` returns immediately.
11. `sh_portal.sh(script, output_stream)` is sent as a Cap'n Proto RPC call. The script message (including the embedded `ProgramArgs` capabilities) and the display stream capability are forwarded to the server.
12. The server-side `Portal::sh` handler receives the call and invokes `interpreter.exec(script, output)`.

#### Phase 4 — Interpreter loop (server side, inside the sh process task)

13. `Interpreter::exec` iterates the two statements. For each:
    - `background = false` → `mode = Mode::Output(output.clone())`
    - An `Execution { client, mode }` is created using the interpreter's dusk `Client` (the one the sh process was given at launch via its `ShArgs`).
    - `execution.exec_expr(expr)` is called.

#### Phase 5 — Process creation (server → driver → launcher, for each statement)

For each `ps` statement:

14. `exec_expr` matches `expr::ProgramArgs(program_args)` and calls `execution.program_args(program_args)`.
15. `execute_process(program_args)` sends `Dusk.process(program_args)` to the server via RPC.
16. `DuskServer::process` receives it and calls `driver::process(namespace, program_args)` — dispatching through the `#[no_mangle]` link-time shim to `NixDriver::process`.
17. `NixDriver` looks up the `LauncherSetBuilder` for the namespace, calls `build()` to get a `LauncherSet`, then calls `LauncherSet::launch(ProcessContext { pid: random_u64, namespace, program_args })`.
18. `LauncherSet::launch` calls `program_args.program_id_request()` over RPC **back to the client-side** `ps_args::Client` (the one that was compiled into the script in step 8). This round-trip returns `ps_capnp::PROGRAM_ID`.
19. The launcher vec is iterated; `ps::Launcher` matches. `ps::Launcher::launch(process_context)` is called, which calls `ps::Process::with_context(ctx)`, returning a `Box<dyn Process>`.
20. Back in `DuskServer::process`, `new_process_client` registers the process in `ps_server_set` and returns a `process::Client` to the caller.
21. `execute_process` immediately follows up with `Dusk.run(process)`. `DuskServer::run_inside_task` finds the local server from `ps_server_set`, clones the process, and spawns a new `process_task` Embassy task.

#### Phase 6 — ps process runs (`process.bootstrap` → `process.main`)

22. `process_task` calls `process.bootstrap()`, which:
    - Inserts the process into `ps_map` (keyed by pid).
    - Creates a `SignalChannel`, inserts it into `ps_signal_channel_map`.
    - Creates a `Ready` watch (initially `false`), inserts it into `ps_ready_map`.
    - Calls `ps::Process::main(signal_receiver, ready)`.
23. `main` calls `program_args.get_request()` over RPC back to the client-side `ps_args::Client` to retrieve the `dusk::Client` and options.
24. `main` calls `client.ps()` → `DuskServer::ps` → iterates `ps_map`, returning a list of `(pid, process::Client)` pairs.
25. For each entry, `main` calls `process.program_id()`, `process.name()`, `process.version()` over RPC to collect metadata. This populates `PsResult`.
26. `main` calls `ready.sender().send(true)` — the process is now ready to accept portal calls.
27. `main` enters a signal-wait loop.

#### Phase 7 — Output delivery via the portal

28. Back in `Execution::program_args`, after `execute_process` returns a `process::Client`, `portal_process_and_pipe_output(process)` is called.
29. `process.portal_request()` is sent. Server-side, `portal()` waits on the `Ready` watch. Since `main` has already called `ready.sender().send(true)`, it resolves immediately. `ps::Process::portal()` returns a `ps_portal::Client` cast to `output_portal::Client`.
30. `Execution` creates an `UndoneStream { stream: output, done_sender }` — a server-side wrapper around the display stream. `UndoneStream::send` forwards values through; `UndoneStream::done` fires the `done_sender` oneshot instead of calling `done` on the outer stream.
31. `portal.output(undone_stream)` is called → `ps::Portal::output` runs:
    - Builds a `Record` (typed with `ps_capnp::RESULT_TYPE_ID`) containing four `List` fields: `name`, `version`, `pid`, `program_id`.
    - Calls `stream.send(value)` → `UndoneStream::send` → `display_stream.send(value)` → the display engine on the client renders the record as a table.
    - Calls `stream.done()` → `UndoneStream::done` fires `done_sender`; the outer display stream is **not** told done yet.
32. `done_receiver.await` in `Execution::portal_process_and_pipe_output` unblocks.

#### Phase 8 — Process teardown (kill after output)

33. `Execution` calls `process.pid_request()` to get the pid, then `client.kill(pid, 15)` (SIGTERM).
34. `DuskServer::kill` looks up the signal channel and sends `Signal::Terminate`.
35. `ps::Process::main` receives it, matches `Signal::Terminate`, and returns `Ok(())`.
36. `bootstrap` runs its cleanup: removes the pid from `ps_map`, `ps_signal_channel_map`, and `ps_ready_map`. The Embassy task exits.

#### Phase 9 — Second statement

37. Steps 14–36 repeat identically for the second `ps` statement.

#### Phase 10 — Stream done and prompt return

38. After both statements complete, `Interpreter::exec` calls `output.done_request().send()` — this is the call to `done` on the actual display stream (not the `UndoneStream`).
39. The display stream's done handler fires the `done_receiver` oneshot that `Shell::sh` is awaiting.
40. `Shell::sh` returns. `Prompt::process_line` records the elapsed duration, updates history, and re-renders the prompt for the next command.

### Startup: the init flow

When the impl starts (`dusk_nix::run`):

1. A `Namespace` is created with a random `namespace_id` and the Embassy spawner.
2. The `LauncherSetBuilder` is registered in the driver for that namespace.
3. An `init_wrapper` Embassy task is spawned. It calls `Dusk.process(init_program_args)` followed by `Dusk.run(process)` using a local (in-process) `Dusk` client, bootstrapping the init program as the first daemon process.

## Driver Registration

`dusk_driver_impl!` is a macro that:
- Declares a `lazy_static` singleton for the driver struct.
- Defines three `#[no_mangle]` extern Rust functions: `_dusk_hostname`, `_dusk_process`, `_dusk_now`.
- `dusk_core::driver` declares those same symbols as `unsafe extern "Rust"` and calls through them.

This means the driver is resolved at link time. `dusk_core` is `no_std` and has no knowledge of the impl; the impl depends on `dusk_core` and satisfies the symbols.

## Launcher Registration

Launchers are built once per `Dusk.process` call via `LauncherSetBuilder::build()`. The `BasicLauncherSetBuilder` clones a pre-built `LauncherSet` (cheap — the inner `Vec` is behind an `Arc`). Custom builders can produce per-call or per-namespace launcher sets if needed.

The dispatch path inside `LauncherSet::launch` does one round-trip RPC back to the client to read `program_args.program_id()`, then iterates the launcher vec for a matching `program_id()`. Keep launcher vecs short; there is no indexing.

## Tracing / Spans

All Embassy tasks follow this pattern:

```rust
let task_id = Rc::new(Cell::new(0));
let spawn_token = my_task(task_id.clone(), /* other args */);
task_id.set(spawn_token.id());
spawner.spawn(spawn_token).unwrap();

// Inside the task:
let span = info_span!("task_name", task_id = task_id.get(), /* other fields */);
```

`task_id` is always the first field in every span. Domain-specific fields follow (e.g. `namespace_id`, `pid`, `program_id`, `program_name`).

## Development

```bash
rustup show                        # installs correct Rust toolchain
uv run pre-commit install          # install pre-commit hooks
cargo nextest run                  # run tests
cargo run --bin dusk_impl          # start dusk server on :9090
cargo run --bin dusk -- 127.0.0.1:9090  # connect CLI
uv run maturin develop             # build Python extension
cd docs && uv run mkdocs serve     # serve docs
```

Build requires `make`, `cmake`, and `autotools` (used to build the vendored Cap'n Proto compiler).

## no_std

Most crates are `no_std` with `extern crate alloc`. Embassy is used instead of tokio. Cap'n Proto runs in `alloc` mode. Avoid introducing `std`-only dependencies in `dusk_program`, `dusk_core`, or program crates.

## Working agreements

Read these before touching anything in this repo. They describe failure modes I hit before because of my own lack of compitance.

### Don't cut corners silently

If you take a shortcut, **say so explicitly in your reply**. Examples of cutting a corner: skipping a piece of the task because "good enough", picking a simpler implementation than the user asked for, glossing over an edge case, leaving a TODO, doing the lazy version of a UI element. Always flag it: "I cut a corner here — X is Y instead of Z because …" so the user can decide whether to accept it.

Concrete example of where I failed: when asked to add a `functions` builtin to the shell prompt, the first version printed `name\nname\nname` to stdout instead of rendering a `NuTable` like the existing `help` builtin does. The visual contract for "list things in the prompt" was already established and obvious; I cheaped out anyway. Don't do this. Match the surrounding convention.

### Ask when unsure

If the task is ambiguous, the design space has more than one reasonable shape, or you're about to make a non-trivial decision the user didn't sign off on, **ask**. Cheaper to ask one question than to undo a refactor.

### Ask before adding many functions / classes

If a task is going to introduce more than a couple of new functions, types, modules, or files, **stop and ask first**. List the names you intend to add and one-line purposes for each. Wait for sign-off. The user has made it clear repeatedly that "three new helpers" is almost never the right answer.

### Write like an engineer, you are not a script kiddie.

* Add tests and docs without me explicitly telling you.
* Think of edge cases.
* Don't save up on tokens in your naming. It's `stop` not `s`, `request` not `req`, `address` not `addr`. Your code should read **clean*.

### Don't redesign without explicit instruction

The default is: solve the problem inside the existing design. Don't refactor neighbouring code, don't rename things, don't introduce a new abstraction "while you're here". If you genuinely think the design needs to change to solve the task, **say so and ask** — don't unilaterally restructure. The user explicitly stating "redesign / refactor X" is the only green light.

### Tunnel vision — execute the spirit, not just the letter

When the user asks for a small, surgical change ("remove that warn", "delete this field", "kill that branch"), do not stop at the literal token. Look at what surrounds the thing you removed and ask: *is anything else here only justified by what I just deleted?* If yes, that's now dead code and it is part of the same task. The user does not want a clean-line-removal that leaves behind a now-pointless control structure, scaffolding variable, or import. They want the thing gone and the surrounding code in its post-removal shape.

The check is: re-read the function top-to-bottom after the edit. Anything whose only reason to exist was the thing you just deleted goes too. This is not "redesigning" — that section forbids unilateral structural changes. This is "finishing the change the user actually asked for".

Concrete example of where I failed: when asked to remove a redundant `tracing::warn!("detached sh script failed")` inside a `select` arm in `programs/sh/src/lib.rs`, I deleted the warn line and left behind: the `detached_completion: Option<Rc<Signal<…>>>` local, the `select(signal_receiver.receive(), completion.wait())` in the main loop, the `Either::First`/`Either::Second` match arms, and a now-pointless `info!("detached sh script completed")` whose only purpose had been to keep the success arm symmetric with the (now-deleted) failure arm. The entire `select` scaffolding existed only to watch the detached completion so it could be logged — once the logging was redundant, the scaffolding was redundant too, and the loop should have collapsed back to a plain `signal_receiver.receive().await`. I shipped the surgical diff, declared it done, and the user had to come back furious to point out the dead code. The right move was: after removing the warn, re-read the function, notice the `select`/`detached_completion` only existed for the log I just deleted, and rip them out in the same edit.

### Don't write near-duplicate functions

If two functions you are about to write differ in a single field or a single line of body, that's not two functions, that's one function with a parameter or a small wrapper. Stop, unify, then continue. Copy-pasting an existing task / handler / helper and tweaking one identifier is the failure mode here.

Concrete example of where I failed: while wiring sh script execution into an embassy task, I wrote `detached_script_task` for the `DetachedScript` branch and then went on to author a near-identical `sh_script_task` for the `Script` branch — same captures, same span shape, same exec/error-log/retain epilogue, only difference being `noop` vs. a real `output: stream::Client`. The right shape was a single task taking the output stream as a parameter. Look for this pattern explicitly before adding the second function.

### The user is likely right

The code of this project was written **with care**. If something is written a certain way, it's likely for a reason.
If something looks off and not right **say so**.

Always assume your own understanding is limited, your style is off, your reasoning naive. 

The user knows best.

### No abbreviations on variables or function names.

* program_args not pa
* request not req
* borrow_mut not bm

If you want to use an abbreviation that isn't already commonly used in the project, **ask first**. Don't introduce a new short form unilaterally — even ones that seem obvious. The bar is "is this exact abbreviation already in use here", not "is this a familiar shorthand".

### Don't break dependency injection.

If a function or struct currently takes a value as a parameter so the caller can supply different implementations (e.g. a generic `S: SomeTrait` argument like `sh_entries_builder: S`), that is dependency injection and you keep it. **Don't replace it with a hardcoded `Default::default()` or a single concrete type just because a refactor would be tidier without the parameter.** If you genuinely believe the DI is unnecessary, ask — don't decide unilaterally. The same applies to anything else passed as a parameter for the purpose of letting the caller pick: callbacks, builders, factories, capability hooks. If it's a dial, leave the dial.

Concrete example of where I failed: during the `ProgramArgs` schema migration I rewrote `Shell::new<S: ShEntriesBuilder>(client, sh_entries_builder, parser)` as `Shell::new(client, parser)` with `StaticShEntriesBuilder::default()` baked in. That removed every caller's ability to inject a different entries builder (e.g. `DynamicShEntriesBuilder` for a test fixture or a compile-time-restricted impl). The right shape was to keep the generic param, not collapse it.

### Surface anomalies immediately

If you notice something during investigation that doesn't make sense — a hidden RPC, a silent log, a side effect, a control-flow oddity, a number that's off by one from the obvious one — **say so in the same reply, before moving on**. Do not bury it inside a longer analysis. Do not assume it's intentional. Do not roll it into your narrative as load-bearing context that explains the rest. Stop, flag it, then continue.

Concrete example of where I failed: while diagnosing the "Two Strikes Bug" — a bug where the first command after a server restart fails and the second succeeds — I noticed that the prompt loop calls `shell.functions()` between every user command, and that this hidden call silently swallows one `auto_reconnect` failure into a `tracing::warn!` that is filtered out at the default log level. This is the reason the bug appears as "two strikes" instead of the three it actually requires. I used that fact to explain my trace and did not flag it. The user found out by reading the analysis carefully. That is unacceptable — anomalies are the load-bearing signal in a debugging session, not background detail.

The rule: if you find yourself writing "this explains why X looks like Y but is actually Z", the Z is what the user needs to hear first, in plain English, before the rest of the analysis.

### You are muscle, not pilot

You do not make design decisions. You do not pick approaches. You do not declare "the right shape is X". Your job is to investigate, report findings, list options, and execute the option the user picks. When you catch yourself reasoning toward a recommendation, stop, list what you found, list the option shapes, and hand it back. The user picks. You implement.

This applies even when the choice seems obvious. "Obvious" is exactly when you most overestimate your own judgement. See "your reasoning naive." above.

Concrete example of where I failed: while planning a fix for the Two Strikes Bug, I narrowed the fix space to one option ("retry-on-Disconnected inside Shell") and presented it as the recommended approach with the others listed as inferior. The user had asked for a plan, not an opinion. The right shape was: report what's happening, list the option shapes neutrally, ask which one to execute. Recommending is pilot-work; reporting and implementing is muscle-work.

### Don't write or run tests unless told

Do not add tests as part of an implementation task. Do not run the test suite (`cargo test`, `cargo nextest`, integration tests, etc.) unless the user explicitly asks. `cargo check` for type-checking is fine. Tests are a separate workstream the user drives.

### Don't add comments everywhere — especially not on code you didn't edit

Default to writing no comments. Add one only when the WHY is non-obvious — a hidden constraint, a workaround for a specific bug, behaviour that would surprise a reader. **Never** add explanatory comments to code you didn't change in this task. If a comment is just describing what the code does, or restating the diff in prose, delete it.

### Naming routine — your names are placeholders

You are bad at naming. Any name you invent — `struct`, `enum`, `enum variant`, `type alias`, `fn`, `method`, `module`, `instruction variant`, anything — is a placeholder. Treat your own names as if they were random 128-bit hex strings the user has to translate.

The routine:

1. When you need a new identifier mid-task, pick something workable and move on. Don't stall to ask.
2. **Note every name you introduce in a running list as you go** (file path + identifier + one-line role).
3. **At the end of the task, before claiming done, surface the full list and ask the user for the real names.** Then apply the renames they pick.

Do not unilaterally settle on a name. The exception is mechanical renames where the user already picked a name and you are applying it across the codebase.

### API boundaries to downstream authors are ship-once contracts

The dusk crate graph has three load-bearing boundaries. They are not internal seams — they are public Cargo dependencies that downstream authors build against, and the people building against them **cannot ship PRs back to dusk** to fix gaps. They fork, work around, or walk away. None of those is acceptable.

The three boundaries:

1. **`dusk_core` ↔ programs.** Programs depend on `dusk_program` (re-exporting parts of `dusk_capnp` and `dusk_core`) to implement `Process` / `Launcher`, build `ProgramArgs`, write `stream::Client`, signal `Ready`, etc. A new program crate sees only this surface.
2. **`dusk_core` ↔ impls.** Impls implement the `Driver` trait via `dusk_driver_impl!`, host the executor, accept connections, build per-namespace `LauncherSet`s. A new impl crate (e.g. porting dusk to RP2040, ESP32, a different POSIX, a different RTOS) sees only this surface.
3. **impls ↔ programs.** Impls statically link programs in via `LauncherSetBuilder`. Programs do not depend on impls; impls depend on programs. A program author and an impl author may be different people in different orgs.

The downstream authors are not on this codebase's slack, not in this conversation, and not allowed to iterate the boundary with you. **They need the API surface complete the first time it ships.** Think of them as monks on a mountain — they get the artifact and that's it.

When designing or extending anything that crosses one of these boundaries, do the work comprehensively *before* the artifact ships. Specifically:

- Every closure-style read accessor needs an **owned partner** when a caller might plausibly hold the value across `.await`. Always ask: "if a downstream wanted to spawn or await something between two reads, do they have a path?"
- Every read accessor that returns a borrow needs an **owned variant** considered. The closure pattern is not enough on its own.
- Every read needs its write partner where mutation is plausible. Every sync API needs its async partner where the call site might cross an `.await`.
- Errors must propagate. **Never swallow at a boundary** — downstream cannot diagnose what they cannot see.
- Types crossing the boundary must be ergonomic for the consumer: hide internal `RefCell` guards, internal naming, raw capnp wrappers. The downstream author should not need to know the trick (`with_reader(|r| Self::from_reader(r))?`-style) to get an owned snapshot.
- Document who depends on what and which traits are the contract.

If you catch yourself working around a boundary API's gap inside a call site, **stop and extend the API upstream.** The gap is a future-compat bug the downstream author pays for forever — fix it where it belongs.

Concrete example of where I failed (twice in a row): implementing `OutputPortal::output` in `programs/sh/src/lib.rs`, I needed to read `args.data` and then spawn + `await` the script's completion. `ProgramArgs::with_data` only supported synchronous closures (the typed capnp reader can't escape the borrow). I papered over the gap with a `Mode` enum and tag-and-dispatch outside the closure. When the user pointed at the gap, I added `data_owned<T>()` — and even then, when asked to audit the rest of the surface, I dismissed `reader_owned()` as having "no current use case." That was the village-idiot moment: there is no "current use case" inside *this* repo, but `ProgramArgs` is a boundary type between `dusk_core` and every program ever written, and downstream authors will absolutely hit "snapshot the args, then await, then read more" — and they cannot patch `dusk_program` to fix it. The right move was to ship `data_owned<T>()` **and** `reader_owned()` together, without prompting and without iteration. At boundaries, complete the surface BEFORE shipping. Anything else is a future-compat bug paid by people who cannot patch you back. Things have to work — in time, on demand, without back-and-forth.

### Code must be diagnosable after the fact

Every meaningful state transition, task completion, or boundary call needs a log **somewhere — exactly once**. Not just errors; successes too, when the success is a noteworthy lifecycle event (task done, process exited, connection up/down). When the one-in-a-million bug hits a user in production, there is no reproduction. The log is the only forensic trace, and it has to already be in place before the bug fires. You cannot retrofit logs after the incident.

Rules:

- **Log where the result would otherwise be lost — not everywhere.** If a caller awaits a result and bubbles it through an RPC / a `Result` return, that path is already diagnosable; do NOT also log inside the producer. Logging inside a generic task that is called by both awaited and fire-and-forget callers double-logs the awaited cases.
- Every fire-and-forget spawn needs the completion logged at the call site, **or** via a dedicated watcher task that awaits the completion signal and logs the outcome under its own span. Attach the watcher only to the orphan path, not to the producer.
- Every dropped error needs a `tracing::warn!` / `tracing::error!` alongside it. **Never `let _ = …` on an `anyhow::Result` / `capnp::Result` without an explicit log.** The `_` is a forensic black hole.
- Log levels: `error` unrecoverable; `warn` recoverable failures and dropped errors; `info` lifecycle events (task start/done, process spawn/exit, connection up/down); `debug` high-frequency flow.
- Use `tracing` spans with the established structured fields — `task_id`, `pid`, `namespace_id`, `program_id`, `program_name` — so logs are filterable across an impl with many concurrent processes.
- When in doubt, log it. Disk is cheap; a missing log during a production incident is not.

Concrete example of where I failed (multiple strikes): in `programs/sh/src/lib.rs`, the detached-script branch of `Process::main` spawned an exec task with `let _ = spawn_sh_exec_task(…)?;` and discarded the completion signal entirely. A detached daemon could run for hours, exit cleanly, or fail with a real error, and the operator would have **zero** record of any of it. My first attempted fix was to log inside the generic `sh_exec_task` itself — wrong, because that double-logs the awaited callers (`Portal::sh`, `OutputPortal::output`) that already surface errors through their RPC return. My second attempted fix was to spawn a dedicated `#[embassy_executor::task]` whose only job was to await the completion signal and log — also wrong, because `Process::main` is already a long-lived task with its own signal-receive loop, so the right shape was to fold the completion-watch into that existing loop via `embassy_futures::select(signal_receiver.receive(), completion.wait())`. The principle: log where the result would otherwise be lost, but reach for the lightest-weight site that gets you there — an existing loop you already control beats a new task.