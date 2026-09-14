# Embedding Dusk

This is Dusk's whole reason for existing: **you embed it into software you
already ship, and that software's devices become a managed fleet.** No separate
service to stand up, no rewrite - you link Dusk in, call one function, and you're
done.

Picture the software inside a media player, a fleet of robot vacuums, or a rack
of build machines. Each is a device someone needs to watch and operate. Embed
Dusk and every one of them becomes a **node** you can get
[analytics](../features/analytics.md) from, [diagnose](../features/diagnosis.md),
and drive - at any scale, on anything from a microcontroller to a supercomputer.

## How you embed it

The universal integration is a **C library**. Dusk's node compiles to a static
or shared library with a single-call entry point, so it drops into an app written
in practically any language. See [The C library](../getting-started/guides/embed.md).

> **TODO - embedding is more than the C library.** The C lib is the first
> integration, not the only one. **Native bindings for major languages and
> frameworks** are on the roadmap, so embedding a node becomes idiomatic in each
> ecosystem rather than an FFI call. These pages will grow as those land.

> **TODO - the connection layer.** How an embedded node is then discovered and
> securely reached by a fleet (identity, encryption, transport) is still being
> built. Linking the library and starting a node works today; the secure
> fleet-wide connection story is in progress.

## Why this is the headline

Most users never write a Dusk program or port it to new hardware - that's the
[framework](../customize/index.md), the advanced path. The common case is far
simpler: take what Dusk already does, and put it inside what you already have.
