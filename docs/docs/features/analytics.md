# Analytics

The first thing a fleet needs is to be **understood**: what Dusk is running across
your devices, how those workloads are behaving, and how that changes over time.
Analytics is the payoff most people reach for first after embedding Dusk.

> **TODO - in progress.** Fleet-wide analytics tooling is still being built. This
> page describes where it's headed; today you collect the raw signals per node
> with the [diagnosis](diagnosis.md) tools and aggregate them yourself.

## Where it's headed

- A live view of every node - its processes, versions, and health - across the
  whole fleet.
- Metrics and structured logs streamed off each node and rolled up centrally.
- Trends over time, so you can watch a single device - or a whole rollout - drift
  before it becomes an incident.

## What you can do today

Each node already exposes the raw material: process listings (`ps`), program
metadata, and structured log records. Pull them per node through the
[client interfaces](../getting-started/guides/connect-a-client.md) and feed them
into your own store while the managed pipeline matures.
