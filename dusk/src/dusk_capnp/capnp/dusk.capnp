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
}

interface Portal {
  process @0 () -> (result: Process);
}

interface Dusk {
    exec @0 (programArgs: ProgramArgs) -> (result: Process);
    portal @1 (process: Process) -> (result: Portal);
    kill @2 (process: Process) -> ();
    ps @3 () -> (processes :List(Process));
    hostname @4 () -> (result :Text);
}

