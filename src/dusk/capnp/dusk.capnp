@0xace6963097d486d6;

# A simple marker interface used to validate at compile time 
# that the type to pass to `exec` makes sense.
interface ProgramArgs {}

struct ExecResult {
  union {
    pid @0 :UInt64;
    programNotFound @1 :Void;
    programCreationFailed @2 :Void;
  }
}

enum KillStatus {
  success @0;
  processNotFound @1;
}

struct Process {
  pid @0 :UInt64;
  program_args @1 :ProgramArgs;
}

struct ProcessResult {
  union {
    process @0 :Process;
    processNotFound @1 :Void;
  }
}

interface Server {
  process @0 () -> (result: ProcessResult);
}

interface Dusk {
    exec @0 (program_args: ProgramArgs) -> (result: ExecResult);
    serve @1 (pid :UInt64) -> (server: Server);
    kill @2 (pid :UInt64) -> (status: KillStatus);
    ps @3 () -> (processes :List(Process));
    hostname @4 () -> (hostname :Text);
}

