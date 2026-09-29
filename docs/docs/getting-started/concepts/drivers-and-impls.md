# Drivers & Impls

A Dusk node is a portable core (`dusk_core`) plus a thin, platform-specific
**impl**. The impl supplies a **driver**: the handful of primitives the hardware
forces Dusk to ask for. This split is why one fleet can span microcontrollers and
supercomputers - the programs and the policy are shared; only the driver changes.

## The `Driver` trait

`Driver` is a small `Send + Sync` trait of OS/hardware hooks:

- **`hostname()`** - what this node calls itself,
- **`exit(exit_code)`** - halt the node.

An impl implements `Driver` and registers it once with `dusk_driver_impl!`.

## The extern-shim pattern

`dusk_core` is `no_std` and depends on **no** impl, yet it has to call into one.
It does this through a link-time shim. `dusk_driver_impl!` defines a `lazy_static`
singleton for the driver plus `#[no_mangle]` extern functions - `_dusk_hostname`
and `_dusk_exit`. `dusk_core::driver` declares those same symbols as
`unsafe extern "Rust"` and calls through them. The linker resolves the symbols to
whichever impl is linked into the final binary. Callers in `dusk_core`, programs,
and other `no_std` crates only ever name `dusk_core::driver::*` and get whatever
impl is present - which is exactly what lets a program like `sleep` be written
once and run under the nix impl today, an MCU impl tomorrow.

## Drivers do not call themselves

There is one trap worth stating plainly: inside a `Driver` impl, do **not** reach
for `dusk_core::driver::hostname()` (or any other `driver::*`). That call
round-trips through the extern shim straight back into your own crate. The shim is
the route *into* the impl for `no_std` callers; the impl is the destination. From
inside the impl, call the underlying OS/hardware primitive directly.

## Impls stay lean

`dusk_core` owns every piece of policy that can be platform-agnostic - schedulers,
queues, state machines all belong there, behind a thinner primitive exposed
through `Driver`. An impl should own only what the platform forces: the hostname,
how to halt the node, the program set to launch, the facts about itself, its
platform and its device it writes into the
[key-value store](../../features/kvs.md#what-the-impl-records) as the node starts,
plus the platform's `embassy-time` driver and
`critical-section` implementation. If you find yourself
adding a non-trivial state machine to an impl, that is a sign the logic belongs in
`dusk_core` instead.

## Time driver

`embassy_time::Timer` needs an `embassy-time-driver` providing `_embassy_time_now`
and `_embassy_time_schedule_wake`. **`dusk_core` does not provide one** - each
impl pulls in the driver appropriate for its platform. The nix impl enables
`embassy-time/std` (a ready POSIX driver); an MCU impl would enable its HAL's
time-driver feature. A program that just wants to sleep uses
`embassy_time::Timer::after(...)` directly; reading or setting a node's
wall-clock goes through the `Dusk.time` / `Dusk.settime` RPCs, not the `Driver`.
