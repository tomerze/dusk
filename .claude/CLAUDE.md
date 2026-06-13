# Dusk

Dusk is a platform for managing **fleets** — collections of machines you operate
as one, anywhere from tiny microcontrollers to supercomputers. Every machine in
the fleet runs a small Dusk **node**, and you drive a node the same way no matter
what hardware it is: connect to it, see what's running, start and stop work, and
read results back.

That single interface spans such different hardware because almost all of Dusk is
platform-agnostic. A node is a portable core plus a thin, per-platform **impl**
that supplies only what the hardware forces — a clock, a way to spawn work, a
hostname. The programs you run on a node are written once and run on every impl.

If you're new here, the user-facing docs under `docs/` are the gentle
introduction. This file is the orientation map for working *in* the codebase.

## The big picture

Dusk has three layers, plus the clients that drive it:

- **Dusk Core** (`dusk/src/`) — the portable runtime and the SDK that programs
  and impls build against. Knows nothing about any specific platform.
- **Programs** (`base/`) — the units of work a node can run: the shell `sh`,
  `ps`, `kill`, `sleep`, and so on. Each is written against Dusk Core and is
  platform-independent.
- **Impls** (`impls/`) — platform backends. `dusk_nix` is the Linux impl;
  packaged as the `dusk_node` server, it's what you actually deploy.
- **Clients** — the `dusk` CLI, the `dusk_py` Python extension, and an MCP
  gateway. Each connects to a node over Cap'n Proto RPC and drives it.

The boundaries between these layers are real public contracts — see
[API boundaries](#api-boundaries-to-downstream-authors-are-ship-once-contracts)
in the working agreements.

## Repository layout

```
dusk/src/      Core crates and client crates (dusk_core, dusk_capnp,
               dusk_program, dusk_program_proc, dusk_prompt, dusk_shell,
               dusk_cli, dusk_py, dusk_build)
base/          The built-in programs (sh, ps, kill, sleep, date, hostname,
               true, false, init, logs)
impls/nix/     The Linux impl (Embassy executor, the NixDriver)
artifacts/     Deliverables you ship: dusk_node (server), dusk (CLI), dusk_py
docs/          The documentation site (MkDocs)
vendor/        External libs submodules
```

## The crates

| Crate | Role |
|-------|------|
| `dusk_capnp` | The Cap'n Proto schemas (`dusk.capnp`, `stream.capnp`) — the wire format every client and node speaks. |
| `dusk_program` | The SDK a program implements: the `ProcessMixin` / `LauncherMixin` traits, `Namespace`, `ProgramArgs`, `Signal`, `Ready`, and the stream helpers. |
| `dusk_program_proc` | The proc macros that remove the boilerplate: `metadata!`, `derive(Args)`, `impl_args_rpc_server`, `derive(Launcher)`, `derive(Process)`, `derive(Portal)`, `impl_portal_rpc_server`. |
| `dusk_program_sh` / `dusk_program_sh_proc` | The shell-entry registry: `ShEntry`, the link-time `SH_ENTRIES` slice, and the `#[sh_entry]` attribute that makes a program shell-invocable. |
| `dusk_core` | The runtime: the `DuskServer` behind the `Dusk` capability, the `Driver` trait and its extern shim, sessions, and the `init` wiring. `no_std`. |
| `dusk_nix` | The Linux impl: hosts the Embassy executor, implements `NixDriver`, enables `embassy-time/std`, and binds the TCP listener. |
| `dusk_prompt` / `dusk_shell` | The interactive shell client — the prompt UI and the `Shell`/`Connection` that drive a long-lived `sh` process. |
| `dusk_cli` | The `dusk` CLI binary (package `dusk_cli_bin`, bin `dusk`). |
| `dusk_py` | The Python extension (the `dusk` module, built with maturin). |
| `dusk_build` | Build-script helpers for compiling `.capnp` schemas. |

The deployable server is `dusk_node` (package `dusk_node_bin`, bin `dusk_node`),
which links Dusk Core, the Base programs, and one impl.

## Core concepts

### Nodes and namespaces

A **node** is one running Dusk server — one machine in the fleet. Internally a
node is exactly one **namespace**: the execution context that holds everything
running on it. A node creates its namespace once at startup, and **every client
session shares it** — two clients connected to the same node see the same
processes. There is no per-connection isolation; you isolate workloads by running
them on separate nodes.

A namespace is executor-local: it's held as an `Rc<Namespace>`, owns the Embassy
spawner, and guards its internal tables with `embassy_sync` mutexes
(`CriticalSectionRawMutex`). It is not `Send`/`Sync`, and all access happens on
the node's single executor.

### Programs and processes

A **program** is a static unit of work compiled into a node — there is no dynamic
loading. A **process** is a running instance of a program. `ps` lists the
processes running on the node you're connected to; `kill` signals one by pid;
`waitpid` waits for one to exit.

### The `Dusk` capability

When a client connects, it receives a `Dusk` capability — the node's whole API:

| Method | What it does |
|--------|--------------|
| `process(programArgs)` | Build a process from its args; returns a `Process` handle. |
| `run(process)` | Spawn a process as its own task (a daemon that outlives the session). |
| `ps()` | List the processes on the node. |
| `kill(pid, signal)` | Send a signal to a process. |
| `waitpid(pid)` | Wait for a process to exit. |
| `hostname()` | The node's hostname. |
| `time()` / `settime(ms)` | Read or set the node's wall-clock. |
| `programs()` | Enumerate the programs the node can run. |

### Portals and streams

A running process exposes its API through a **portal** — a typed Cap'n Proto
capability. At the core, `Dusk.Portal` carries only the program id; each program
**extends** it with the methods clients actually call. A client calls
`process.portal()` (which resolves once the process is ready), reads the program
id, and downcasts to the concrete portal type.

Processes move data over **streams** (`Dusk.Stream`), which carry `Value`s — a
schemaless-but-typed union (`uint`, `string`, `text`/Markdown, `bytes`, `bool`,
`list`, `record`). A `Record` carries a type id plus `(key, value)` fields, which
is how a program returns structured, tabular output without a compiled schema on
the wire. `Stream`, `Value`, and `Record` all live in `dusk.capnp`;
`stream.capnp` holds only the `StreamResult` streaming shim.

### The driver

Everything platform-specific lives behind the `Driver` trait
(`Send + Sync`), which an impl implements:

- `hostname()` — the node's hostname.
- `exit(exit_code)` — halt the node.
- `launchers(namespace)` — build the node's `LauncherSet` (the set of programs it
  can run) for a namespace.

An impl registers its driver once with `dusk_driver_impl!`. See
[Driver registration](#driver-registration-the-extern-shim-pattern).

## Anatomy of a program

Every program is five parts, tied together by a one-line `metadata!`:

```rust
dusk_program_proc::metadata!("sleep", VERSION, sleep_capnp::PROGRAM_ID);
```

1. **Program id** — a `u64` constant in the program's `.capnp` file. It's the
   dispatch key the `LauncherSet` matches on.
2. **Args** — a capnp struct with a nested `Data` (startup data) and a `Server`
   interface (client-side callbacks). These become the `D` and `S` of the core
   `ProgramArgs(D, S)`. In Rust: `#[derive(Args)]` with a `#[data]` field, and
   `#[impl_args_rpc_server]` to host the `Server`.
3. **Launcher** — the factory. `#[derive(Launcher)]` generates its identity
   (`program_id`, `version`, `git_rev`); you write `LauncherMixin::launch`, which
   takes a `ProcessContext` and returns a `Box<dyn Process>`.
4. **Process** — the async runtime. A struct with a `#[process_context]` field
   and `#[derive(Process)]`, plus a `ProcessMixin` impl providing `with_context`,
   `main(signal_receiver, ready)`, and `portal()`.
5. **Portal** — the typed capability, a capnp interface extending `Dusk.Portal`.
   In Rust: `#[derive(Portal)]` plus `#[impl_portal_rpc_server]`.

A process's `main` does its work, sends `true` on its `ready` watch when it can
accept portal calls, then waits on `signal_receiver` (a `DynamicReceiver<Signal>`)
— returning on `Signal::Terminate` (wire value 15).

To make a program runnable from the shell by name, its client side adds a
`#[dusk_program_sh_proc::sh_entry] pub fn sh_entry() -> ShEntry` with the command
name and help text. (See `base/sleep` for a clean, complete example.)

## Calling back into the client

A program's args are `ProgramArgs(D, S)`, and the `S` half is how a running
program **calls back into the client**. `data` is one-way startup data the client
sends in; `server` is a live capability **hosted on the client** that the program
can invoke from the node. Server-side, a program pulls it out of its
`ProcessContext` and calls it like any capability — each call is an RPC that runs
back on the client:

```rust
let server: my_capnp::my_args::server::Client =
    process_context.program_args.server_as()?;
// server.some_method_request()… runs on the client
```

`#[impl_args_rpc_server]` is what hosts that `Server` interface on the client
side. So data flows through a process in both directions:

- **client → program** — the typed [portal](#portals-and-streams) and its streams.
- **program → client** — the `Server` capability carried in the args.

The shell is the clearest example. `ShArgs.Server` exposes
`buildProgramArgs(command)`; when the `sh` interpreter on a node compiles a
command word, it calls *back to the client* to build that command's
`ProgramArgs` — because the program registry and each command's client-side
capabilities live on the client. A `Script` running on the node therefore
resolves every command against the connected client. This callback channel is
also why the connection has to stay live while a script runs.

## Driver registration (the extern-shim pattern)

`dusk_core` is `no_std` and depends on no impl, yet it must call into one. It does
so through a link-time shim. `dusk_driver_impl!` defines a `lazy_static` singleton
for the driver plus `#[no_mangle]` extern functions — `_dusk_hostname`,
`_dusk_exit`, `_dusk_launchers`. `dusk_core::driver` declares those same symbols
as `unsafe extern "Rust"` and calls through them. The linker resolves them to
whichever impl is in the final binary.

**Callers in `dusk_core`, programs, and other `no_std` crates never name an
impl** — they go through `dusk_core::driver::*` and get whatever impl is linked.
That's what lets a program like `sleep` be a single crate that runs under nix
today and an MCU impl tomorrow.

**Drivers do not call themselves.** Inside a `Driver` impl, do not reach for
`dusk_core::driver::*` — that round-trips through the shim straight back into your
own crate. The shim is the route *into* the impl for `no_std` callers; the impl is
the destination. Call the OS/hardware primitive directly.

**Impls stay lean.** `dusk_core` owns every piece of policy that can be
platform-agnostic. An impl owns only what the platform forces: the hostname, how
to halt, the program set to launch, and the platform's `embassy-time` driver and
`critical-section` implementation. A queue, scheduler, or state machine sneaking
into an impl is a sign the logic belongs in `dusk_core` behind a thinner
primitive.

## How a command runs

**A client session.** When a client connects, the node's `init` process accepts
the connection and spawns a `session` task that shares the node's single
namespace and hands the client a `Dusk` capability (a `DuskServer` exposed as the
Cap'n Proto bootstrap capability). The transport underneath is being reworked, so
don't lean on its specifics.

**`Dusk.process` → run.** `Dusk.process(programArgs)` asks the driver for the
node's `LauncherSet` (`driver::launchers(namespace)`) and dispatches: it reads
`program_args.program_id()` — a **local** read of the in-memory args message, not
a network call — and runs the matching launcher's `launch`, which returns a
`Box<dyn Process>`. The client then chooses the process's lifetime:

- `Dusk.run(process)` spawns it as its own task — it outlives the session.
- `process.run()` runs it inside the calling session.

Either way the process is entered through `bootstrap`, which registers it in the
namespace and cleans it up when `main` returns.

**Portals and kill.** `process.portal()` waits for the process's `Ready` watch,
then returns the portal for the client to downcast. `Dusk.kill(pid, signal)` looks
up the process's signal channel in the namespace and sends the signal, which the
process receives on its `signal_receiver`.

**Startup.** `dusk_node_run()` calls into `dusk_nix::run`, which creates the
`Namespace`, registers the impl's `LauncherSet` builder, and spawns `init`. `init`
binds the listener and accepts connections.

The deepest end-to-end trace (a `ps; ps` shell line, from keystroke to spawned
process) lives in `docs/docs/development/shell.md`.

## The shell is a program

`sh` is just another program — but it's where a lot of behaviour that people
mistake for "core" actually lives. **Daemonization** (a process kept alive by
*not* acknowledging a stream's `done`), the `output(stream)` portal method
(`OutputPortal`), `sh -d` detached scripts, the shell language and its
`Script`/function/interpreter machinery, `ShStop`, and the `SH_ENTRIES` registry
are all part of `sh`, not of Dusk Core. When documenting or reasoning about the
core process model, keep these on the shell side of the line.

## Artifacts and clients

The crates under `artifacts/` are example **deliverables** — templates you copy,
add your own programs to, and ship. They're also the answer to "how do I actually
run and talk to a node."

### `dusk_node` — the server, three ways

`dusk_node` packages Dusk Core, the Base programs, and an impl into a runnable
node. Its body is tiny — `default_launcher_set()` builds every Base program at
its default configuration (building the logs launcher inside it also installs
the global tracing subscriber, since `dusk_node` enables the logs program's
`console` feature), and `dusk_nix::run` starts the node with an `init` bound to
`0.0.0.0:9090`:

```rust
pub extern "C" fn dusk_node_run() -> i32 {
    let Ok(launcher_set) = dusk_base::default_launcher_set() else {
        return 1;
    };
    let Ok(init_args) = InitArgs::new("0.0.0.0", 9090).as_program_args() else {
        return 1;
    };
    dusk_nix::run(dusk_nix::BasicLauncherSetBuilder::new(launcher_set), init_args)
}
```

For custom launcher arguments (e.g. a different `LogsConfig`), skip
`default_launcher_set` and assemble the set yourself with
`LauncherSet::from_launchers`.

Because its crate type is `["rlib", "staticlib", "cdylib"]`, you can consume it
three ways:

- **As a binary** — `dusk_node_bin` wraps it as the `dusk_node` executable
  (`cargo run --bin dusk_node`).
- **As a C library** — the `staticlib`/`cdylib` expose one entry point, declared
  in `artifacts/dusk_node/include/dusk.h`:
  ```c
  int32_t dusk_node_run(void);
  ```
  Link `libdusk_node` and call `dusk_node_run()` to run a node and get its exit
  code. Dusk drops into an existing C/C++ program with no Rust on the surface.
- **As a Rust rlib** — call `dusk_node::dusk_node_run()` directly, or copy its
  body to assemble your own node (different programs, different impl).

### `dusk` — the CLI

`dusk_cli` builds the `dusk` binary. Point it at a node and either drop into the
interactive shell or run a single command:

```bash
dusk 127.0.0.1:9090            # interactive prompt
dusk 127.0.0.1:9090 "ps"       # run one command and exit
```

It's a thin layer over the Rust client path: it drives a long-lived `sh` process
behind an interactive prompt.

### `dusk_py` — the Python extension

`dusk_py` (a cdylib named `dusk`, built with maturin) exposes a node to Python:

```python
import dusk
node = dusk.Dusk('127.0.0.1', 9090)   # connects; blocks until the node answers
print(list(node.sh('ps')))            # sh(command) → iterator of output values
node.disconnect()
dusk.Dusk.help()                      # static: enumerate programs, no connection
```

### The Rust SDK — talking to programs directly

Rust callers can drive a node *below* the shell's text interface. Once you hold
the node's **`Dusk` capability**, you call the `Dusk` API directly — `process`,
`run`, `ps`, `kill`, … — and for a rich program you take the process's `portal()`
and **downcast it to that program's typed portal client** to call its real
methods (e.g. `ShPortal.sh(...)`) instead of sending command text. Each program
crate's `client` feature provides the building blocks: typed portal clients, args
builders, and the client-hosted `Server` [callbacks](#calling-back-into-the-client).

This is the lower-level path for complex programs: hold the `Dusk` capability,
build typed `ProgramArgs`, call typed portals, and field the program's callbacks —
the same surface the CLI and Python bindings are built on.

> How a client establishes a connection and obtains that `Dusk` capability is
> intentionally left undocumented here — the transport/connection layer is slated
> for rework (encryption among other things), so anything written now would go
> stale.

## The client / server split (`no_std` vs `std`)

**The server is `no_std`. The client is `std`.** A program crate holds both
sides, separated by a `client` Cargo feature:

- **Server side** — the unconditional code, compiled into a node. It must be
  `no_std`-clean: `extern crate alloc`, no `std::` imports, no std-only deps in
  the unconditional `[dependencies]`. Use `alloc::rc::Rc`, `alloc::vec::Vec`,
  `alloc::string::String`, `core::cell::*`.
- **Client side** — anything behind `#[cfg(feature = "client")]` (usually
  `client.rs`), compiled into the CLI and Python extension. `std`, `clap`, and
  friends are fine here; std-only deps are `optional = true` and activated by the
  `client` feature.

The same rule holds for the non-program crates: `dusk_core` and `dusk_program`
are `no_std`; impls and the client crates (`dusk_prompt`, `dusk_shell`,
`dusk_cli`) are `std`. A quick sanity check:
`grep -rn "std::\|use std" base/ --include="*.rs"` should only hit `client.rs`
files or `#[cfg(feature = "client")]` modules.

## Development

```bash
rustup show                              # installs the pinned toolchain
uv run pre-commit install                # install pre-commit hooks
cargo nextest run                        # run the tests
cargo run --bin dusk_node                # start a node on :9090
cargo run --bin dusk -- 127.0.0.1:9090   # connect the CLI
uv run maturin develop                   # build the Python extension
cd docs && uv run mkdocs serve           # serve the docs site
```

Building requires `make`, `cmake`, and `autotools` for the vendored Cap'n Proto
compiler under `vendor/`.

### Embassy and conventions

Dusk runs on a **single-threaded [Embassy](https://embassy.dev) async executor**.
That choice is what lets the same runtime work on a bare-metal microcontroller and
on hosted Linux. A few consequences shape almost everything you write:

- **Executor-local, not multi-threaded.** Futures are `!Send`; the `Process`
  trait is `#[async_trait(?Send)]`. Shared state uses `Rc` / `RefCell` and
  `embassy_sync` mutexes (`CriticalSectionRawMutex`), never `Arc` + an OS lock.
  Concurrency within a node is cooperative — long awaits should yield.
- **Tasks.** Long-lived activities (sessions, processes) each run as their own
  Embassy task, spawned through the namespace's `Spawner`.
- **Time.** Timing comes from `embassy-time`. `dusk_core` ships **no**
  `embassy-time-driver`; each impl supplies one (nix enables `embassy-time/std`),
  along with a `critical-section` implementation. To sleep, use
  `embassy_time::Timer::after(...)`; to read or set a node's wall-clock, use the
  `Dusk.time` / `Dusk.settime` RPCs.

**Tracing.** Every Embassy task opens a `tracing` span. `task_id` is always the
first field; domain fields follow (`namespace_id`, `pid`, `program_id`,
`program_name`), so logs stay filterable across many concurrent processes.

```rust
let task_id = Rc::new(Cell::new(0));
let spawn_token = my_task(task_id.clone(), /* … */);
task_id.set(spawn_token.id());
spawner.spawn(spawn_token).unwrap();

// inside the task:
let span = info_span!("task_name", task_id = task_id.get(), /* … */);
```

**Naming and comments.** Spell identifiers out — `request`, not `req`; `address`,
not `addr` — even for short-lived locals. Default to no comments; add one only
when the *why* is non-obvious. These and the rest of the behavioural rules live in
the working agreements below, which take precedence over everything else here.

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

## Terminology lives on one side of a boundary

Code is named for what its own side does and knows — never for what the other side does with the output. If a stated invariant says "X doesn't know about Y", then no module name, identifier, doc, or comment on X's side may say Y. When writing or reviewing code at a boundary, check the names against the invariants written beside them — grep the side for the other side's vocabulary.

Past failure: the node side of the `logs` program was a module named `view.rs` — "the node side of a view subscription", a `view_result` local, "(the viewer quit)" comments — while the same crate's schema said, verbatim, "the node doesn't know" where entries land. The name had leaked in from the user-facing `logs view` subcommand; the node's half of the feature is just streaming. Review read the prose invariant and the identifiers in the same pass and never collided them, because naming wasn't treated as part of the contract under review.

## Say the domain's word — no imported metaphors

When the codebase already has the exact term for a mechanism, prose, comments, and locals use that term. The logs transfer is a **stream** (`Dusk.Stream`, `StreamResult`, `stream_task`, capnp-streaming): say *streaming*, not "pumping". Mechanic/plumbing words — pump, plumbing, glue, wire up — are banned wherever a domain term exists.

Past failure: the same module described itself as "pumping" in five places and the docs called the callback path "the plumbing", while every identifier around them said `stream`. Worse: while renaming `view` → `stream` I edited the very doc line containing "pumping" and kept the word, because I had scoped the fix as a hunt for the token "view" instead of a vocabulary audit of every line I touched. A terminology fix is a sweep: grep the crate *and* the docs for the whole family of wrong words before claiming done.

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

## Never touch the git index — it is the user's

Never run anything that writes the staging area: `git add`, `git reset`, `git mv`, `git rm`, `git stash`, `git restore --staged`, or any other index-mutating command. The user curates staging by hand; it is not yours to change, ever — not even transiently "to undo it after." To see a diff that includes untracked files, **read the files directly** — do not `git add` to make them show up in `git diff`. `git status` and `git diff` (read-only) are allowed; anything that mutates the index is not.

Past failure: to pull untracked files into a `git diff --stat`, I ran `git add -A` and then `git reset` to "undo" it. That reset flattened the user's entire hand-curated staging state down to HEAD. No file content was lost — a mixed reset leaves the working tree untouched — but the staged/unstaged split they had built was destroyed, could not be reconstructed, and the user had to rebuild it by hand. This was real disruption, not a near-miss. The index is sacrosanct: strictly read-only.

I have caused great harm.

