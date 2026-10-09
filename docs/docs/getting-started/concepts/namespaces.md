# Namespaces

A **namespace** is a node's execution context - the container for every
[process](processes.md) running on that node. One running node is exactly one
namespace. When you connect to a node in your fleet, you are talking to its
namespace.

As a program author you never construct or manage a namespace directly; the
runtime hands you a reference to it. A process receives it inside its
`ProcessContext`, and an impl receives it in the driver's `launchers` hook.
Conceptually, the namespace is the thing that tracks the node's running
processes (what `ps` reports), routes signals to them (`kill`), and hands out the
capabilities clients call.

## One namespace per node

A node creates its namespace once, at startup, and **every client session shares
it**. Two clients connected to the same node see the same processes - there is no
per-connection isolation. Isolation between workloads is achieved by running them
on separate nodes, not by carving up a single node's namespace.

## Executor-local

A namespace runs on a single-threaded Embassy executor. It is held as an
`Rc<Namespace>` and owns the executor's spawner, so it is pinned to that
executor - it is not `Send`/`Sync`, and all access to it happens on the node's
executor. The internal bookkeeping it guards (process tables, signal channels,
readiness watches) is runtime machinery; see the
[Architecture](../../development/architecture.md) page if you need that detail.
