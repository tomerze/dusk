# Build a custom impl

Writing an impl is the deepest **framework** use of Dusk - you reach for it only
to run nodes on hardware no existing impl covers. It's optional and rare, but it's
what makes Dusk **modular**: the platform backend is swappable without touching a
single program.

An **impl** is what makes Dusk run on a particular class of hardware. Porting
Dusk to a new platform - a new MCU, a new OS - means writing an impl. The programs
in your fleet don't change; only the impl does. For the background on this split,
read [Drivers & Impls](../concepts/drivers-and-impls.md) first.

## When you need a custom impl

You need a new impl when you want nodes on hardware no existing impl covers. If
an impl for your platform already exists (the nix impl covers Unix-like systems,
the windows impl covers Windows, and the std impl covers everything else
with the Rust standard library), you don't - you just link your programs into a
node built on it.

## Implement the `Driver`

Provide the platform primitives Dusk asks for - `hostname()` and
`exit(exit_code)` - and register them once with `dusk_driver_impl!`. `exit`
records the exit code and returns; your impl runs its executor with
`Executor::run_until` and stops when the code is set, as `impls/std` does. It
must not panic or block. The
macro defines the `#[no_mangle]` extern symbols `dusk_core` calls through; the
linker connects the two.

Remember the rule: inside the driver, call the OS/hardware primitive directly -
never route back through `dusk_core::driver::*`, which would loop straight back
into your own crate.

## Provide a time driver

`dusk_core` ships no `embassy-time-driver`. Your impl must enable one for its
platform - `embassy-time/std` on a hosted target, or a HAL's time-driver feature
(`embassy-stm32/time-driver-tim2`, `embassy-rp/time-driver`, …) on an MCU. You
must also supply a `critical-section` implementation (e.g. `critical-section/std`),
which the Embassy mutexes in `dusk_program` rely on.

## Register launchers

Decide which programs nodes on this impl can run by building a per-namespace
[`LauncherSet`](../concepts/launchers.md) (for example with
`LauncherSet::from_launchers(vec![...])`), and hand `dusk_core::init::init` a
closure returning it. `init` puts it in the registry and takes it out again when
the namespace terminates, so registration is not something an impl does or has
to undo. The registry lives in `dusk_core`, not in the driver.

## Host the executor

Finally, run the Embassy executor, create the node's
[namespace](../concepts/namespaces.md), and spawn `init` with an init script -
a `Bytecode.Bytecode` message it runs through `sh`. Hand it `nightfall -l 0.0.0.0:9090` and
the node accepts client sessions. This is the
[startup sequence](run-a-node.md#what-happens-at-startup)
the nix impl follows. Keep the impl lean: anything that isn't a forced platform primitive
belongs in `dusk_core`, not here.
