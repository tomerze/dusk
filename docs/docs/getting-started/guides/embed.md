# Embed Dusk in your app

The most common way to use Dusk is to **embed it into an application you already
ship**. Your app keeps doing what it does; Dusk rides along and turns every device
running it into a manageable node in a fleet.

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

`dusk_node` builds as a static C library, with its header at
`artifacts/dusk_node/lib/include/dusk.h`:

```c
#include "dusk.h"

int64_t dusk_new(void);
int32_t dusk_run(int64_t namespace_id, void *user);
int64_t dusk_spawn(void *user);
int32_t dusk_join(int64_t namespace_id);
void dusk_free(int64_t namespace_id);
```

A node is named by its namespace id: a positive `int64_t`. Every function that
returns one returns 0 when it fails.

Link `libdusk_node` and call `dusk_spawn()` in your app's startup. It starts a
node on a thread of its own - on port 9090, by default - and returns its id, or 0
if it could not draw an id or start a thread for the node:

```c
int64_t namespace_id = dusk_spawn(NULL);
```

`dusk_join` waits for a node `dusk_spawn` started to end, returns its result, and
then unregisters its id. A spawned id stays registered until it has been joined.

A node ends when its `init` process ends - today, when a client terminates `init`
(`kill <pid>`). There is no call to stop a node from C yet.

To run the node on a thread you choose, split `dusk_spawn` in two. `dusk_new`
registers an id and returns it, or returns 0 if it cannot draw one from the
operating system's random source. `dusk_run` runs the node under that id on the
calling thread until the node shuts down, and returns its result:

```c
int64_t namespace_id = dusk_new();
if (namespace_id != 0) {
    int32_t result = dusk_run(namespace_id, NULL);
}
```

`dusk_free` unregisters an id `dusk_new` returned that has not been run. It does
nothing for any other id.

### The result

`dusk_run` and `dusk_join` return one `int32_t`. Its low 8 bits are Dusk's
status, and the bits above them are the node's exit code - today 0 when its
`init` ends, and -1 if the node panicked:

```c
int32_t status = result & DUSK_STATUS_MASK;
int32_t exit_code = result >> DUSK_EXIT_CODE_SHIFT;
```

A node that ran and exited with 0 returns 0. The exit code has 24 bits, signed,
so an exit code outside -8388608 to 8388607 loses its top bits.

| status | meaning |
|---|---|
| `DUSK_RUN_OK` | the node ran and exited; the exit code is in the high bits |
| `DUSK_RUN_LAUNCHERS_FAILED` | the node's launcher set could not be built |
| `DUSK_RUN_INIT_ARGS_FAILED` | the init script's program arguments could not be built |
| `DUSK_RUN_UNKNOWN_NAMESPACE` | the id was never registered, or its run has ended: its node exited or failed to start |
| `DUSK_RUN_ALREADY_RUNNING` | a node is running under the id, `dusk_spawn` started it, or it is being joined |
| `DUSK_RUN_NOT_SPAWNED` | `dusk_join` of an id `dusk_spawn` did not start |

An id runs once. A `dusk_run` that claims it - one that returns anything but
`DUSK_RUN_UNKNOWN_NAMESPACE` or `DUSK_RUN_ALREADY_RUNNING` - unregisters it
before returning, whether its node ran or failed to start; a spawned id is
unregistered by the `dusk_join` that joins it, once the join is done.

From Rust, link the `dusk_node` rlib and call the same functions -
`dusk_node::dusk_spawn()`, or `dusk_node::dusk_new()` and `dusk_node::dusk_run()` -
and set
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

`dusk_run` hands `user` to the node it runs on the calling thread, and
`dusk_spawn` hands it to the node on the thread it starts. It must stay valid for
as long as that node runs, and a node that reads it does so from the node's own
thread.

> **TODO:** node identity and the encrypted transport are part of the
> connection-layer work, and neither is configurable yet, through `user` or
> otherwise. This guide will grow as that lands.

## What it gives you

Once your app is a node, point a client at it to get the [analytics and
diagnosis](../index.md#what-you-get) Dusk is for - see its processes, read its
logs, and drive it from the [shell](../../features/shell.md), the
[Python API](connect-a-client.md), or the
[API gateway](../../features/gateway.md).
Multiply that across every device running your app and you have a managed fleet.

## Make it yours

The default `dusk_node` links the Base programs and the nix impl. Changing which
programs ship, targeting a different platform, or adding capabilities of your own
is the **framework** side of Dusk - see [Write a program](first-program.md) and
[Build a custom impl](custom-impl.md). It's optional: most integrations run the
defaults.
