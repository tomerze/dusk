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

# interface LsArgs extends(ProgramArgs) {
#   struct ProgramId {
#     programId @0 :UInt64 = 5;
#   }
#   struct Args {
#     filepath @0 :Text;
#   }
# }

struct ExecResult {
  union {
    pid @0 :UInt64;
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
  programArgs @1 :ProgramArgs;
}

struct ProcessResult {
  union {
    process @0 :Process;
    processNotFound @1 :Void;
  }
}

interface Portal {
  process @0 () -> (result: ProcessResult);
}

interface Dusk {
    exec @0 (programArgs: ProgramArgs) -> (result: ExecResult);
    portal @1 (pid :UInt64) -> (portal: Portal);
    kill @2 (pid :UInt64) -> (status: KillStatus);
    ps @3 () -> (processes :List(Process));
    hostname @4 () -> (hostname :Text);
}

