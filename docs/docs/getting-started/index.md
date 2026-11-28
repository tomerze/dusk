# Getting Started

Dusk gives a system **fleet management** - analytics, diagnosis, and remote
control of every node - without you having to build that infrastructure
yourself.

## How you use Dusk

Most often, you **embed Dusk into an application you already ship**. Dusk's node
compiles to a small C library; you link it into your app, call one function, and
every running copy of that app becomes a manageable node in a fleet. Think of a
mobile app, a game, an agent harness, or the software inside a robot vacuum -
each is a member of a fleet someone needs to observe and operate. Drop the Dusk
library in, and that's done.

Dusk runs in other shapes too:

- as a **standalone application**, a node on its own,
- as a small **embedded operating system** in its own right,
- and as a **framework** for writing the programs and platform backends that run
  on a node.

But the headline is embedding: you bring Dusk to the software you already have.

> **Vision ahead of implementation - TODO:** the embed-and-connect workflow
> depends on the connection/transport layer, which is still being built. Parts of
> what follows describe the intended experience ahead of that work landing - they're
> marked where they outrun what's shippable today.

## What you get

Once your software and hardware are Dusk nodes, three things open up - roughly
in the order
people reach for them:

1. **Analytics** - understand your fleet: what's running, how it's behaving,
   trends across many nodes.
    > **TODO:** fleet-wide analytics tooling is in progress. Today you can gather
    > raw data per node with the programs below.
2. **Diagnosis** - inspect and fix a misbehaving nodes: list its processes
   (`ps`), read its logs, signal or restart work (`kill`), or poke at it
   interactively through the [shell](../features/shell.md).
3. **A framework** - when the built-ins aren't enough, extend Dusk with your own
   [programs](concepts/programs.md) and [impls](concepts/drivers-and-impls.md).
   Most people never reach for this - like nobody writes their own SSH server,
   though they could - but it's there, and it's what makes Dusk **modular**: every
   program and every platform backend is swappable.

## The mental model

Anything running embedded Dusk - an app, a game, an agent, a device - is a
**node**: internally one [namespace](concepts/namespaces.md) holding the
[processes](concepts/processes.md) running on it. You connect to a node, see
what's running, and drive it. The pieces that differ from one platform to the
next live behind a [driver](concepts/drivers-and-impls.md), so the same tooling
works across every node in the fleet.

## Where to go next

- **[Embedding](../embedding/index.md)** - put Dusk inside an app you already
  ship. The usual starting point.
- **[Connect a client](guides/connect-a-client.md)** - reach a node to see and
  drive it (analytics and diagnosis).
- **[Customize](../customize/index.md)** - the framework: extend Dusk with your
  own programs and platform backends. The advanced path.
