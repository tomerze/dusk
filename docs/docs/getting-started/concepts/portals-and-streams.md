# Portals & Streams

A running [process](processes.md) exposes its API through a **portal** — a typed
Cap'n Proto capability — and moves data over **streams**.

## Portals

At the core, a portal is the `Dusk.Portal` interface, which carries only the
program id:

```capnp
interface Portal {
  programId @0 () -> (program_id :UInt64);
}
```

A program **extends** `Portal` with the methods clients actually call. A client
obtains a portal with `process.portal()` — which resolves once the process is
[ready](processes.md#readiness) — reads its `programId`, and downcasts the
capability to the program's concrete portal type.

> Programs that produce output to the shell extend the shell's `OutputPortal`
> (its `output(stream)` method). That is a convention of the
> [shell](../../features/shell.md), layered on top of `Dusk.Portal` — it is not
> part of the core portal.

## Streams

A `Stream` is a process's stdin/stdout. It is defined in `dusk.capnp`:

```capnp
interface Stream {
  send @0 (value :Value) -> stream;   # streaming return → automatic back-pressure
  done @1 () -> ();                   # idempotent: later calls do nothing
}
```

`send` carries a `Value`. Its streaming return lets the RPC layer apply
back-pressure: a fast producer is throttled to the rate the consumer drains.
`done` marks the end of the stream. It is idempotent, and it has to be: a
program may end a stream early, `sh` ends it when a line of shell is over, and
a stream nobody ended is ended when its server object is dropped. Every call
after the first must succeed and do nothing.

## Values

A `Value` (also defined in `dusk.capnp`) is a schemaless-but-typed union covering
`null`, `uint`, `string`, `text` (interpreted as Markdown), `bytes`, `bool`,
nested `list`s, and `record`s.

## Records

A `Record` carries a `typeId` and a list of `(key, value)` fields. The type id
names the struct shape, so a client can recognise and render structured, tabular
output without any compiled schema on the wire — this is how a program like `ps`
returns its results.
