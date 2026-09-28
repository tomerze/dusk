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

`dusk_node` builds as a static C library, with a one-function
header (`artifacts/dusk_node/lib/include/dusk/dusk.h`):

```c
#include <dusk/dusk.h>

int32_t dusk_node_run(void *user);
```

Link `libdusk_node` and call `dusk_node_run()` - typically on its own thread,
since it runs the node until shutdown:

```c
// in your app's startup
dusk_node_run(NULL);   // a node on port 9090; returns an exit code
```

From Rust, link the `dusk_node` rlib and call
`dusk_node::dusk_node_run()`, and set
[`DUSK_NODE_INIT_SCRIPT`](../../embedding/node-artifacts.md#building-with-cargo)
for the build. From any other language, bind the C function.
That's the whole integration: one library, one call.

Which impl you build it with depends on where your application runs - `impl_nix`
here, `impl_windows` on Windows, `impl_std` everywhere else. See
[Node artifacts](../../embedding/node-artifacts.md).

## The `user` pointer

`dusk_node` is a template - you copy it, put your programs and your impl in it,
and ship the result, so `dusk_node_run` is a function in **your** library. See
[Make it yours](#make-it-yours).

Editing the template settles what your node is *built from*. `user` is the other
half: what your application knows only once it is **running**. It is the one
channel from the program running the node into the node, and what it points at
is between the two of them - Dusk itself never looks at it.

The template itself ignores `user`, so `NULL` is the only call it needs. It is in
the signature for the node you build from it: your node can read the pointer as a
config struct, a device handle, a callback table, or the identifier the device was
provisioned with.

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
