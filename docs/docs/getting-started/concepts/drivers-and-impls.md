# Drivers & Impls

A Dusk node is a portable core (`dusk_core`) plus a thin, platform-specific
**impl**. The impl supplies a **driver**: the handful of primitives the hardware
forces Dusk to ask for. This split is why one fleet can span microcontrollers and
supercomputers - the programs and the policy are shared; only the driver changes.

## The `Driver` trait

`Driver` is a small `Send + Sync` trait of OS/hardware hooks:

- **`hostname()`** - what this node calls itself,
- **`tid()`** - which thread is calling: the same number for the whole life of a
  thread, and a different one for every thread running a node at the same time.
  The logs program uses it to send each record to the node running on that
  thread. An impl with one node and no threads returns the same number every
  time,
- **`fs_driver()`** - the node's file system, as a `Box<dyn FsDriver>`,
- **`exit(exit_code)`** - halt the node.

An impl implements `Driver` and registers it once with `dusk_driver_impl!`.

## The file system

`fs_driver()` hands a program the node's file system through two
`#[async_trait]` traits in `dusk_core::driver`, modelled on the operations of
Plan 9's 9P without speaking it:

| `FsDriver` | |
|---|---|
| `open(path, mode)` | Open a file and return it as a `Box<dyn File>`. `OpenMode` says whether to `read`, `write`, `create` it if it is missing, and `truncate` it. |
| `stat(path)` | A `Stat`: the `name`, `length` in bytes, whether it `is_directory`, its `mode` bits and when it was `modified`, in milliseconds since the Unix epoch. |
| `remove(path)` | Remove a file or an empty directory. |
| `rename(from, to)` | Rename a file or directory. |
| `create_dir(path)` | Create a directory. |
| `read_dir(path)` | The `Stat` of every entry in a directory. |

| `File` | |
|---|---|
| `read(offset, buffer)` | Read into `buffer` from `offset`, returning how many bytes were read; `0` at the end of the file. |
| `write(offset, data)` | Write `data` at `offset`, returning how many bytes were written. |
| `stat()` | The file's `Stat`. |
| `truncate(length)` | Cut or extend the file to `length` bytes. |
| `sync()` | Make every write so far durable. |

Reads and writes name their offset, as 9P's do; a `File` has no cursor, so two
tasks can use one without moving each other's position.

Every future these traits return is `Send`, as `Driver` is. The file systems of
the three impls in this repository call `std::fs` inside those futures, so a
read or write runs on the executor's thread until the disk answers - a slow disk
holds up every other task on the node for that long. The nix impl reads and
writes with `pread` and `pwrite`; the std and Windows impls seek and read under a
lock. The std and Windows impls have no Unix permission bits, and report a
`mode` of `0o444` for a read-only file and `0o666` for any other.

## The extern-shim pattern

`dusk_core` is `no_std` and depends on **no** impl, yet it has to call into one.
It does this through a link-time shim. `dusk_driver_impl!` defines a `lazy_static`
singleton for the driver plus `#[no_mangle]` extern functions - `_dusk_hostname`,
`_dusk_tid`, `_dusk_fs_driver` and `_dusk_exit`. `dusk_core::driver` declares those same symbols as
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
