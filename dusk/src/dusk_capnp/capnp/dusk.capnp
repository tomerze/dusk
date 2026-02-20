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

# Program arguments struct inherit from here,
# it is later downcasted based on programId.
interface ProgramArgs {
  programId @0 () -> (program_id: UInt64);
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

interface Dusk {
    process @0 (programArgs: ProgramArgs) -> (result: Process);
    run @1 (process: Process) -> ();
    ps @2 () -> (process_entries :List(ProcessEntry));
    kill @4 (pid: UInt64, signal: UInt64) -> ();
    hostname @3 () -> (result :Text);
}
