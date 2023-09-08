@0xace6963097d486d6;

struct ExecResult {
  union {
    pid @0 :Int32;
    programNotFound @1 :Void;
  }
}

enum KillStatus {
  success @0;
  processNotFound @1;
}

struct Process {
  pid @0 :Int32;
  program @1 :Text;
}

interface Dusk {
    exec @0 (program :Text, args :List(Text)) -> (result: ExecResult);
    kill @1 (pid :Int32) -> (status: KillStatus);
    ps @2 () -> (processes :List(Process));
    hostname @3 () -> (hostname :Text);
}

