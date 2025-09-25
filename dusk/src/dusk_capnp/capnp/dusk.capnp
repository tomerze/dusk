@0xace6963097d486d6;

struct Field {
  key @0 :Text;
  value @1 :Value;
}

struct Value {
  union {
    null @0 :Void;
    int @1 :Int64;
    uint @2 :UInt64;
    text @3 :Text;
    bytes @4 :Data;
    bool @5 :Bool;
    fields @6 :List(Field);
    list @7 :List(Value);
  }
}

interface Stream {
  send @0 (value :Value) -> stream;
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
