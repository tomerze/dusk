# Cap'n Proto Schemas

The wire format every Dusk client and node speaks. The schemas live in the
`dusk_capnp` crate under `dusk/src/dusk_capnp/capnp/` and are the contract an
out-of-tree client builds against. This page describes `dusk.capnp` and
`stream.capnp` as committed.

## `dusk.capnp`

### `Dusk`

The bootstrap capability a client receives on connecting to a node. Its methods
are the whole top-level surface:

```capnp
interface Dusk {
  process   @0 (programArgs :ProgramArgs(AnyPointer, AnyPointer)) -> (result :Process);
  run       @1 (process :Process) -> ();
  ps        @2 () -> (process_entries :List(ProcessEntry));
  kill      @4 (pid :UInt64, signal :UInt64) -> ();
  hostname  @3 () -> (result :Text);
  waitpid   @5 (pid :UInt64) -> ();
  time      @6 () -> (unix_time_ms :UInt64);
  settime   @7 (unix_time_ms :UInt64) -> ();
  programs  @8 () -> (program_entries :List(ProgramEntry));
}
```

- `process` builds a process from args; `run` spawns it as a daemon. (Running a
  process inside the caller's session uses `Process.run` instead.)
- `ps` / `kill` / `waitpid` operate on running processes by pid.
- `time` / `settime` read and set the node's wall-clock; `hostname` and
  `programs` describe the node and the program set linked into it.

### `Process`

A handle to a running process:

```capnp
interface Process {
  pid       @0 () -> (result :UInt64);
  programId @1 () -> (result :UInt64);
  name      @2 () -> (result :Text);
  version   @3 () -> (result :Text);
  run       @4 () -> ();
  portal    @5 () -> (result :Portal);
}
```

`portal` resolves once the process is ready and returns its `Portal`.

### `Portal`

The base of every process's public API — it carries only the program id, and each
program **extends** it with typed methods:

```capnp
interface Portal {
  programId @0 () -> (program_id :UInt64);
}
```

A client downcasts a `Portal` to the program's concrete portal type using the
program id. See [Portals & Streams](../getting-started/concepts/portals-and-streams.md).

### `ProgramArgs(D, S)`

The arguments handed to a program. It is a **generic struct**, not an interface:

```capnp
struct ProgramArgs(D, S) {
  programId @0 :UInt64;
  args :group {
    data   @1 :D;   # startup data
    server @2 :S;   # a client-side capability (callbacks)
  }
}
```

A program supplies its own `Data` and `Server` types as `D` and `S` — typically a
`struct Data { union { … } }` of startup data and an `interface Server { … }` of
client-side callbacks.

### `ProcessEntry` / `ProgramEntry`

Returned by `ps` and `programs` respectively: `ProcessEntry` pairs a `pid` with a
`Process`; `ProgramEntry` carries a program's `programId`, `version`, and
`gitRevision`.

## Streams and values

`Stream` and `Value` are defined in `dusk.capnp` alongside the interfaces above;
`stream.capnp` itself contains only `StreamResult` (the streaming-compat shim
described below). `Stream` is a process's stdin/stdout, carrying `Value`s:

```capnp
interface Stream {
  send @0 (value :Value) -> stream;   # streaming return → automatic back-pressure
  done @1 () -> ();
}
```

`send` returns a `StreamResult` (`stream.capnp`) — the placeholder type that makes
the method a streaming method, so the RPC layer applies flow control instead of
acknowledging delivery.

A `Value` is a schemaless-but-typed union:

```capnp
struct Value {
  struct Record {
    struct Field { key @0 :Data; value @1 :Value; }
    typeId @0 :UInt64;            # names the struct shape without a compiled schema
    fields @1 :List(Field);
  }
  union {
    null   @0 :Void;
    uint   @1 :UInt64;            # signed integers are cast
    text   @2 :Text;             # UTF-8, rendered as Markdown
    string @7 :Text;             # UTF-8, plain
    bytes  @3 :Data;
    bool   @4 :Bool;
    record @5 :Record;
    list   @6 :List(Value);
  }
}
```

A `Record`'s `typeId` is how structured output stays typed (a client recognises
the shape) while remaining schemaless (no compiled struct on the wire).

## Program IDs

Every program is identified by a random `u64` constant, declared in its own
`.capnp` file as `const programId :UInt64 = 0x…;`. Launcher dispatch, portal
downcasting, and `ps`/`programs` output all key off this id.

## Using the schemas externally

An out-of-tree client talks to a node by depending on `dusk_capnp` and driving the
`Dusk` capability directly — exactly what the CLI, the Python extension, and the
MCP gateway do. The schemas are the only contract you need; nothing about a
node's impl leaks across the wire.
