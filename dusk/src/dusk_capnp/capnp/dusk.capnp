@0xace6963097d486d6;

interface Stream {
  sendChunk @0 (chunk :Data) -> stream;
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
  kill @4 (signal: UInt64) -> ();
}

interface Portal {}

interface Dusk {
    process @0 (programArgs: ProgramArgs) -> (result: Process);
    run @1 (process: Process) -> ();
    ps @2 () -> (process_entries :List(Process));
    hostname @3 () -> (result :Text);
}
