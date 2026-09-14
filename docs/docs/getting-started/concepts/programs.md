# Programs

> Programs are part of the **framework** layer - the advanced way to use Dusk.
> Most integrations [embed](../../embedding/index.md) a node and run the built-in
> programs without ever writing one. Reach for this when you're
> [customizing Dusk](../../customize/index.md).

A **program** is a unit of work a node can run - something like `ps`, or one of
your own. Programs are static: each is compiled into a node (there is no dynamic
loading), and a client turns one into a running [process](processes.md) by
calling `Dusk.process`.

Every program is made of five parts, plus a one-line `metadata!` declaration that
ties them together:

```rust
dusk_program_proc::metadata!("sleep", VERSION, sleep_capnp::PROGRAM_ID);
```

## 1. Program id

A `u64` constant identifying the program, declared in its `.capnp` file:

```capnp
const programId :UInt64 = 0xc0da1e11ed8d4a36;
```

It is the dispatch key: a node's `LauncherSet` matches an incoming `Dusk.process`
to the launcher with the same id.

## 2. Args

A program's args are a capnp struct with two nested members - startup `Data` and
a `Server` interface of client-side callbacks (often empty):

```capnp
struct SleepArgs {
  struct Data { durationMs @0 :UInt64; }
  interface Server {}
}
```

These supply the two type parameters of the core `ProgramArgs(D, S)` wire type.
On the Rust side you declare an `Args` struct with a single `#[data]` field and
host its `Server`:

```rust
#[derive(dusk_program_proc::Args)]
pub struct Args {
    #[data]
    pub data: ArgsDataBuilder,
}

#[dusk_program_proc::impl_args_rpc_server]
impl Args {}
```

`#[derive(Args)]` packs the data and the `Server` capability into a
`ProgramArgs`; `#[impl_args_rpc_server]` hosts the `Server` interface so the node
can call those callbacks back on the client.

## 3. Launcher

The factory that builds a process from args. Derive its identity and write the
one build method:

```rust
#[derive(dusk_program_proc::Launcher)]
pub struct Launcher;

#[async_trait::async_trait(?Send)]
impl dusk_program::launcher::LauncherMixin for Launcher {
    async fn launch(&mut self, process_context: ProcessContext)
        -> anyhow::Result<Box<dyn Process>> {
        Ok(Box::new(Process::with_context(process_context).await?))
    }
}
```

See [Launchers](launchers.md).

## 4. Process

The async runtime: a struct carrying a `#[process_context]` field, with
`#[derive(Process)]` for the identity methods and a `ProcessMixin` impl providing
`with_context`, `main`, and `portal`. See [Processes](processes.md).

## 5. Portal

The typed capability clients call - a capnp interface extending `Dusk.Portal`:

```capnp
interface SleepPortal extends(Dusk.Portal) {}
```

On the Rust side, `#[derive(Portal)]` serves the base `programId` method and
`#[impl_portal_rpc_server]` hosts the program-specific interface. See
[Portals & Streams](portals-and-streams.md).

## Making a program shell-invocable

The five parts make a program runnable over `Dusk.process`. To also run it from
the [shell](../../features/shell.md) by name, give its client side an
`#[sh_entry]` function that registers the shell entry name and its help text. That
registration is part of the shell, not of the core program model.
