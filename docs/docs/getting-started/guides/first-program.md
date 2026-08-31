# Write a program

Writing your own program is the **framework** side of Dusk — the third way people
reach for it, after [analytics and diagnosis](../index.md#what-you-get). Most
integrations run the Base programs and never need this; you write a program when
you want a node to do something the built-ins don't. It's also where Dusk's
modularity shows: a program is a self-contained unit you drop into a node.

A program is a unit of work a node can run — something like `ps`, or one of your
own. This guide walks the five parts in order, using a `sleep`-style program as
the example. For the reference on each part, follow the links into
[Concepts](../concepts/programs.md).

Every program crate opens with one declaration tying the parts together:

```rust
dusk_program_proc::metadata!("sleep", VERSION, sleep_capnp::PROGRAM_ID);
```

## Define the schema

A program owns a `.capnp` file declaring its program id, an `Args` struct (a
`Data` union plus a `Server` interface), and a `Portal` interface that extends
`Dusk.Portal`:

```capnp
const programId :UInt64 = 0xc0da1e11ed8d4a36;

struct SleepArgs {
  struct Data { durationMs @0 :UInt64; }
  interface Server {}        # client-side callbacks, if any
}

interface SleepPortal extends(Dusk.Portal) {}
```

## Declare the args

```rust
#[derive(dusk_program_proc::Args)]
pub struct Args {
    #[data]
    pub data: ArgsDataBuilder,
}

#[dusk_program_proc::impl_args_rpc_server]
impl Args {}
```

See [Programs › Args](../concepts/programs.md#2-args).

## Write the process

A struct with a `#[process_context]` field, plus a `ProcessMixin` impl. `main`
reads the args, does the work, signals readiness, then waits for signals:

```rust
#[derive(Clone, dusk_program_proc::Process)]
pub struct Process {
    #[process_context]
    pub ctx: ProcessContext,
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    async fn with_context(ctx: ProcessContext) -> anyhow::Result<Self> {
        Ok(Process { ctx })
    }

    fn portal(&self) -> portal::Client {
        let client: sleep_capnp::sleep_portal::Client =
            capnp_rpc::new_client(Portal { process: self.clone() });
        client.cast_to::<portal::Client>()
    }

    async fn main(&self, signal_receiver: SignalReceiver<'async_trait>, ready: Ready)
        -> anyhow::Result<()> {
        let duration_ms = self.ctx.program_args
            .with_data::<sleep_capnp::sleep_args::data::Owned, _, _>(|d| Ok(d.get_duration_ms()))?;
        // … do the work …
        ready.sender().send(true);
        loop {
            match signal_receiver.receive().await {
                Signal::Terminate => return Ok(()),
                _ => {}
            }
        }
    }
}
```

See [Processes](../concepts/processes.md).

## Write the launcher

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

## Write the portal

```rust
#[derive(dusk_program_proc::Portal)]
pub struct Portal {
    pub process: Process,
}

#[dusk_program_proc::impl_portal_rpc_server]
impl Portal {}
```

See [Portals & Streams](../concepts/portals-and-streams.md).

## Make it shell-invocable

To run the program from the [shell](../../features/shell.md) by name, add a
client-side entry that supplies the shell entry name and help text:

```rust
#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry { /* name, short/long description, args builder */ }
```

## Wire it into a node

Finally, add the launcher to the node's program set so nodes can run it — see
[Build a custom impl](custom-impl.md).
