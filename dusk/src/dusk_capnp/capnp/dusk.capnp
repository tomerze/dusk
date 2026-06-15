@0xace6963097d486d6;

# This trick makes our schema backwards compatible with older versions of capnp.
# Capnp implementations treat it like the `stream` keyword introduced in newer capnp versions.
using StreamResult = import "/capnp/stream.capnp".StreamResult;

struct Value {
  struct Record {
    struct Field { 
      key @0 :Data;
      value @1 :Value;
    }
    typeId @0 :UInt64;  # distinguishes struct types, schemaless but typed.
    fields @1 :List(Field);
  }
  union {
    null @0 :Void;
    uint @1 :UInt64; # for signed integers, cast
    text @2 :Text; # UTF-8 string rendered as Markdown
    string @7 :Text; # UTF-8 string
    bytes @3 :Data;
    bool @4 :Bool;
    record @5 :Record;
    list @6 :List(Value);
  }
}

interface Stream {
  send @0 (value :Value) -> StreamResult;
  done @1 () -> ();
}

struct ProgramArgs(D, S) {
  programId @0 :UInt64;
  args :group {
    data @1 :D;
    server @2 :S;
  }
}

interface Process {
  pid @0 () -> (result :UInt64);
  programId @1 () -> (result :UInt64);
  name @2 () -> (result: Text);
  version @3 () -> (result: Text);
  run @4 () -> ();
  portal @5 () -> (result :Portal);
}

interface Portal {
  programId @0 () -> (program_id: UInt64);
}

struct ProcessEntry {
  pid @0 :UInt64;
  process @1 :Process;
}

struct ProgramEntry {
  programId @0 :UInt64;
  version @1 :Text;
  gitRevision @2 :Text;
}

interface Dusk {
    process @0 (programArgs :ProgramArgs(AnyPointer, AnyPointer)) -> (result: Process);
    run @1 (process: Process) -> ();
    ps @2 () -> (process_entries :List(ProcessEntry));
    kill @4 (pid: UInt64, signal: UInt64) -> ();
    hostname @3 () -> (result :Text);
    waitpid @5 (pid: UInt64) -> ();
    time @6 () -> (unix_time_ms :UInt64);
    settime @7 (unix_time_ms: UInt64) -> ();
    programs @8 () -> (program_entries :List(ProgramEntry));
    id @9 () -> (result :UInt64);
}
