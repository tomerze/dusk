# Embed Dusk in your app

The most common way to use Dusk is to **embed it into an application you already
ship**. Your app keeps doing what it does; Dusk rides along and turns every
running copy of it into a manageable node in a fleet.

## The idea

Dusk's node builds as a C library. You link it into your app and call one
function - your app is now a Dusk node, and the standard tooling (analytics,
diagnosis, remote control) works against it. No rewrite, no separate service.

> **TODO:** the end-to-end embed-and-connect experience depends on the
> connection/transport layer (including encryption and node identity), which is
> still being built. The integration below - linking the library and starting a
> node - works today; how a fleet then discovers and securely reaches those nodes
> is the part still in progress.

## Link the library

`dusk_node` builds as a static C library. Its header is Dusk's own,
`dusk/include/dusk.h`, which the `dusk::node` CMake target puts on your include
path. Link `libdusk_node` and call `dusk_spawn()` in your app's startup:

```c
#include "dusk.h"

uint64_t handle = dusk_spawn(NULL, NULL);  // a namespace on port 9090, on its own thread
```

Every call, its result and its lifetime are documented in `dusk.h` itself. In
short:

- `dusk_new` returns a **namespace handle** - 1, 2, 3 and up, never reused, the
  way OpenGL names its objects. `dusk_run` runs a namespace under it on the
  calling thread until it stops; once it has, the handle may be run again.
- `dusk_spawn` runs a namespace on a thread of its own until it stops, then hands
  its result to your `finalize`. It is C in `dusk.h` itself, so it is there only
  under `DUSK_PTHREAD` or `DUSK_WIN32`, which the `dusk::node` target defines
  wherever CMake finds pthreads or Win32 threads.
- A result is one `int64_t`. Put it in a `union dusk_result` to read its
  `source`, a `DUSK_RESULT_SOURCE_*` saying what produced it, and its `value`.
  With `DUSK_RESULT_SOURCE_NAMESPACE` (0), the namespace exited and the value is
  its exit code. With `DUSK_RESULT_SOURCE_DUSK_MAIN`, Dusk failed before the
  namespace started and the value is a `DUSK_MAIN_FAILED_*` saying why. With
  `DUSK_RESULT_SOURCE_RUST_PANIC`, there was a Rust panic.

From Rust, link the `dusk_node` rlib and call `dusk_new` and `dusk_run`, and set
[`DUSK_NODE_INIT_SCRIPT`](../../embedding/node-artifacts.md#building-with-cargo)
for the build. From any other language, bind the C functions.
That's the whole integration: one library, one call.

Which impl you build it with depends on where your application runs - `impl_nix`
here, `impl_windows` on Windows, `impl_std` everywhere else. See
[Node artifacts](../../embedding/node-artifacts.md).

## The `user` pointer

`dusk_node` is a template - you copy it, put your programs and your impl in it,
and ship the result, so `dusk_run` and `dusk_spawn` are functions in **your**
library. See
[Make it yours](#make-it-yours).

Editing the template settles what your node is *built from*. `user` is the other
half: what your application knows only once it is **running**. It is the one
channel from the program running the node into the node, and what it points at
is between the two of them - Dusk itself never looks at it.

The template itself ignores `user`, so `NULL` is the only call it needs. It is in
the signature for the node you build from it: your node can read the pointer as a
config struct, a device handle, a callback table, or the identifier the device was
provisioned with.

`dusk_spawn` takes `user` over, with a `finalize` callback: Dusk calls
`finalize(user, result)` once the namespace has stopped - even if its thread
never starts - so a spawned namespace frees what you gave it and tells you how it
ended.

> **TODO:** node identity and the encrypted transport are part of the
> connection-layer work, and neither is configurable yet, through `user` or
> otherwise. This guide will grow as that lands.

## What it gives you

Once your app is a node, point a client at it to get the [analytics and
diagnosis](../index.md#what-you-get) Dusk is for - see its processes, read its
logs, and drive it from the [shell](../../features/shell.md), the
[Python API](connect-a-client.md), or the
[API gateway](../../features/gateway.md).
Multiply that across every copy of your app and you have a managed fleet.

## Make it yours

The default `dusk_node` links the Base programs and the nix impl. Changing which
programs ship, targeting a different platform, or adding capabilities of your own
is the **framework** side of Dusk - see [Write a program](first-program.md) and
[Build a custom impl](custom-impl.md). It's optional: most integrations run the
defaults.
