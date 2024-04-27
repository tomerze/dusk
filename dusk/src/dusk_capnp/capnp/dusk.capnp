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

struct ExecResult {
  union {
    process @0 :Process;
    programNotFound @1 :Void;
    programLaunchFailed @2 :Void;
  }
}

enum KillStatus {
  success @0;
  processNotFound @1;
}

struct Process {
  pid @0 :UInt64;
  programId @1 :UInt64;
}

struct PortalResult {
  union {
    portal @0 :Portal;
    processNotFound @1 :Void;
  }
}

interface Portal {
  process @0 () -> (result: Process);
}

interface Dusk {
    exec @0 (programArgs: ProgramArgs) -> (result: ExecResult);
    portal @1 (process: Process) -> (result: PortalResult);
    kill @2 (process: Process) -> (status: KillStatus);
    ps @3 () -> (processes :List(Process));
    hostname @4 () -> (hostname :Text);
}

