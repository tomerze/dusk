@0xace6963097d486d6;

# This trick makes our schema backwards compatible with older versions of capnp.
# Capnp implementations treat it like the `stream` keyword introduced in newer capnp versions.
using StreamResult = import "/capnp/stream.capnp".StreamResult;

# How to generate ids for keyId and typeId for Value:
# 1. Take the bytes of the key or type name without a null terminator.
# 2. Hash them using RapidHash V3 Nano with AVALANCHE = true, PROTECTED = true with the below seed.
# 3. Done!
# There is a const function `dusk_program::value::gen_id` which generates ids according to the above.
const idSeed :UInt64 = 0xbcfcb5e7ea7fb6f3;

struct Field { 
  keyId @0 :UInt64; # hash of the field name, used as the key in maps.
  value @1 :Value;
}

struct Value {
  struct Fields {
    typeId @0 :UInt64;  # distinguishes struct types, schemaless but typed.
    entries @1 :List(Field);
  }
  union {
    null @0 :Void;
    uint @1 :UInt64; # for signed integers, cast
    text @2 :Text; # UTF-8 string rendered as Markdown
    string @7 :Text; # UTF-8 string
    bytes @3 :Data;
    bool @4 :Bool;
    fields @5 :Fields;
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
  run @2 () -> ();
  portal @3 () -> (result :Portal);
}

interface Portal {
  input @0 () -> (stream :Stream);
  output @1 (stream :Stream) -> ();
}

struct ProcessEntry {
  pid @0 :UInt64;
  process @1 :Process; 
}

interface Dusk {
    process @0 (programArgs: ProgramArgs) -> (result: Process);
    run @1 (process: Process) -> ();
    ps @2 () -> (process_entries :List(ProcessEntry));
    kill @4 (process: Process, signal: UInt64) -> ();
    hostname @3 () -> (result :Text);
}
