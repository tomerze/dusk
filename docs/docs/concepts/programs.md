# Programs

A dusk program is a static rust library that runs both on the client side and server side.

Think of a program as an executable compiled into your dusk impl, it can't be loaded dynamically
but it's there for you to create a process out of it.

## Components

Programs are made out of 5 components:

* Program Id
* Program Args
* Launcher
* Process
* Portal

### Program Id

A `#!rust u64` representing the program, usually defined inside the capnp definition file. 

For example
```capnp
const programId :UInt64 = 0x8d0e0504ec994ea4;
```

### Program Args

A capability extending `#!capnp Dusk.ProgramArgs` for example
```capnp
interface ShArgs extends(Dusk.ProgramArgs) {}
```

This capability is provided by the dusk client to the dusk launcher (more on launchers later).
At the very fundamental level program args allow you to send arbitrary data and client-side rpc function callbacks to a process when it is created.

### Launcher

A struct implementing the `#!rust dusk_program::launcher::Launcher` trait for example
```rust
pub struct ShLauncher {}

impl dusk_program::launcher::Launcher for ShLauncher {
    fn program_id(&self) -> u64 {
        sh_capnp::PROGRAM_ID
    }
    fn launch(
        &mut self,
        pid: u64,
        namespace: alloc::rc::Rc<dusk_program::namespace::Namespace>,
        program_args: dusk_capnp::dusk_capnp::program_args::Client,
    ) -> anyhow::Result<Box<dyn dusk_program::process::Process>> {
        let cast_program_args =
            capnp::capability::FromClientHook::cast_to::<ShArgs>(program_args);
        Ok(Box::new(ShProcess::new(
            pid,
            namespace,
            cast_program_args,
        )))
    }
}
```

Launchers are initialized once by the dusk impl and are used to `launch` the program into a process.
They allow processes to communicate with each other efficiently and share the same memory.


### Process

A struct implementing the `#!rust dusk_program::process::Process` trait.

Note: This trait is defined with `#!rust #[async_trait::async_trait(?Send)]`

By overriding the `main` method of the trait you can provide a main method to your program.
The default `main` method just runs forever (by awaiting a pending future).

Processes run as long as their main method runs.

To allow communicating with your process you need to override the `portal` method, which returns a 
portal to your process.

### Portal

Portals are the way to interact with processes.

They are capabilities extending `#!capnp Dusk.Portal` for example
```capnp
interface ShPortal extends(Dusk.Portal) {
  sh @0 (command :Text, output :Dusk.Stream) -> (input :Dusk.Stream);
}
```
A dusk client can obtain an instance of the capability by running the `portal` method on the process and downcast it to 
the portal capability based on the program id of the process.

The `#!capnp Dusk.Portal` interface defines method to obtain an input stream and provide and output stream to the portal.
Over these streams you can send `Dusk.Value`s which allow you to send arbitrary structured but schemaless data.
This is effectively the `stdin` and `stdout` of your portal.

## Daemonization

A process is "daemonized" when its parent (typically a shell running it as a command)
relinquishes the ability to clean it up automatically. After daemonization the process
keeps running and is only torn down when something explicitly sends it `Dusk.kill`.

The signal for daemonization is **the absence of a `done` call on the output stream**:

* When the parent calls `output.output(stream)` it expects the process to either:
    1. Drive the stream to completion and call `stream.done()` on it (the stream is
       wrapped server-side, so calling `done` does **not** propagate to the parent's
       stream — it only tells the parent "I'm finished, you can clean me up"). The
       parent then sends `Dusk.kill` and `Dusk.waitpid` to tear the process down.
    2. Return from the `output` handler **without** calling `stream.done()`. The
       parent treats this as "the process intends to keep running" and skips the
       kill / waitpid step entirely. The process is now a daemon.

The wrapping detail matters: the parent installs an `UndoneStream` between itself and
the child so that `done` is intercepted as a "you can clean me up" signal rather than
ending the parent's view of its own stream. This is what lets a long-running daemon
go through the same `output` handshake as a short-lived command.

A few consequences:

* Daemonization is a per-call decision, not a per-process mode. The same `Process`
  type can serve a one-shot call and a daemon call from different `output`
  invocations — it's purely a function of whether the handler chose to ack `done`.
* From the shell's perspective a daemonized command always reports success: the
  command's `Inst::ProgramArgs` returns `Ok` once `output` returns, regardless of
  whether `done` was called.
* Because there is no kill, **the daemon is responsible for its own teardown**.
  It has to either exit on its own (`Signal::Terminate` from a deliberate
  `Dusk.kill` from somewhere else) or run forever.

The shell's `sh -d <command>` is the canonical example: it runs its argument
through a discard sink at process startup, then deliberately omits the `done` ack
in `output` so the resulting `sh` process daemonizes.
