# Proc macros

`dusk_program_proc` (and `dusk_program_sh_proc`) supply the macros that remove
the boilerplate from a [program](../getting-started/concepts/programs.md). This is
the framework toolkit — you only meet these when you're
[customizing Dusk](../customize/index.md). The `sleep` program is a compact
end-to-end example of all of them.

## `metadata!`

```rust
dusk_program_proc::metadata!("sleep", VERSION, sleep_capnp::PROGRAM_ID);
```

Declares the program's identity — its name, version, and program id. The
`derive` macros below read this to generate their identity methods, so you
declare it once and the rest follow.

## `#[derive(Args)]` and `#[data]`

```rust
#[derive(dusk_program_proc::Args)]
pub struct Args {
    #[data]
    pub data: ArgsDataBuilder,
}
```

Builds your `Args` type into the wire `ProgramArgs(D, S)` — packing the `#[data]`
field as the data half and your `Server` interface as the capability half.

## `#[impl_args_rpc_server]`

```rust
#[dusk_program_proc::impl_args_rpc_server]
impl Args {}
```

Hosts the program's `Server` interface on the client side. This is what lets a
running program [call back into the client](../getting-started/concepts/portals-and-streams.md)
through the `server` capability in its args.

## `#[derive(Launcher)]`

```rust
#[derive(dusk_program_proc::Launcher)]
pub struct Launcher;
```

Generates the launcher's identity methods (`program_id`, `version`, `git_rev`)
from `metadata!`. You still write the one build method, `LauncherMixin::launch`,
which turns a `ProcessContext` into a `Box<dyn Process>`. See
[Launchers](../getting-started/concepts/launchers.md).

## `#[derive(Process)]` and `#[process_context]`

```rust
#[derive(Clone, dusk_program_proc::Process)]
pub struct Process {
    #[process_context]
    pub ctx: ProcessContext,
}
```

Generates the process's identity methods (`pid`, `program_id`, `name`, `version`)
from the `#[process_context]` field. You supply the behaviour by implementing
`ProcessMixin` (`with_context`, `main`, `portal`). See
[Processes](../getting-started/concepts/processes.md).

## `#[derive(Portal)]` and `#[impl_portal_rpc_server]`

```rust
#[derive(dusk_program_proc::Portal)]
pub struct Portal { pub process: Process }

#[dusk_program_proc::impl_portal_rpc_server]
impl Portal {}
```

`derive(Portal)` serves the base `Dusk.Portal` `programId` method;
`impl_portal_rpc_server` hosts your program-specific portal interface. See
[Portals & Streams](../getting-started/concepts/portals-and-streams.md).

## `#[sh_entry]`

```rust
#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry { /* name, descriptions, args builder */ }
```

Registers the program in the link-time `SH_ENTRIES` table so the
[shell](../features/shell.md) can run it by name, carrying its short and long help
text. It's the one macro from `dusk_program_sh_proc`, and it lives on the
program's client side.
