# Launchers

A **launcher** is the factory that turns a request to run a program into a live
[process](processes.md). Programs are compiled into a node but inert; the
launcher is what instantiates one on demand when a client calls `Dusk.process`.

## What a launcher does

A launcher implements `dusk_program::launcher::Launcher`. It declares the program
id it serves and, given a `ProcessContext` - the new `pid`, the node's
[`namespace`](namespaces.md), and the caller's `program_args` - produces a
`Box<dyn Process>`. Inside `launch` it downcasts the generic args to the
program's concrete args type and constructs the process.

## Defining a launcher

`#[derive(dusk_program_proc::Launcher)]` generates the launcher's identity methods -
`program_id`, `version`, `git_rev` - from the program's `metadata!`. You then
write the one method that builds the process:

```rust
#[derive(dusk_program_proc::Launcher, Default)]
pub struct Launcher;

#[async_trait::async_trait(?Send)]
impl dusk_program::launcher::LauncherMixin for Launcher {
    async fn launch(&mut self, process_context: ProcessContext)
        -> anyhow::Result<Box<dyn Process>> {
        Ok(Box::new(MyProcess::new(process_context)))
    }
}
```

The process type carries a `#[process_context]` field, so the `ProcessContext` is
wired into it.

## How dispatch works

A node holds a `LauncherSet` - a list of launchers. When `Dusk.process` arrives,
`LauncherSet::launch` reads `program_args.program_id()` - a **local** read of the
in-memory args message, not a network call - then walks the launcher vec for a
matching id and calls its `launch`. There is no index or hash; dispatch is a
linear scan, so launcher vecs are kept short.

## Registration

An impl builds a node's `LauncherSet` - typically with
`LauncherSet::from_launchers(vec![...])` - and hands it back through the driver's
`launchers(namespace)` hook. Which programs a node can run is exactly which
launchers that set contains. See [Build a custom impl](../guides/custom-impl.md).
