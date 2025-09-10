@0xace6963097d486d6;

interface Stream {
  sendChunk @0 (chunk :Data) -> stream;
  done @1 () -> ();
}

# A simple marker interface used to validate at compile time 
# that the type to pass to `exec` makes sense.
interface ProgramArgs {
  programId @0 () -> (program_id: UInt64);
}

interface Process {
  pid @0 () -> (result :UInt64);
  programId @1 () -> (result :UInt64);
  run @2 () -> ();
}

interface Portal {
  process @0 () -> (result: Process);
}

struct ProcessEntry {
  pid @0 :UInt64;
  programId @1 :UInt64;
}

interface Dusk {
    process @0 (programArgs: ProgramArgs) -> (result: Process);
    run @1 (process: Process) -> ();
    portal @2 (pid: UInt64) -> (result: Portal);
    kill @3 (pid: UInt64) -> ();
    ps @4 () -> (process_entries :List(ProcessEntry));
    hostname @5 () -> (result :Text);
}
