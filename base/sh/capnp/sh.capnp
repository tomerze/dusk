@0xb25a041190c0e845;

using Dusk = import "/capnp/dusk.capnp";
using Bytecode = import "/capnp/bytecode.capnp";

const programId :UInt64 = 0x8d0e0504ec994ea4;
const defaultPid :UInt64 = 0xf2efce60e8c425d0;

struct ShArgs {
  struct Data {
    union {
      server @0: Void;
      script @1: Bytecode.Script;
      detachedScript @2: Bytecode.Script;
      prompt @3: Text;
    }
  }
  interface Server {
    compiler @0 () -> (result :Compiler);
  }
}

interface Compiler {
  buildProgramArgs @0 (command :Text)
    -> (programArgs :Dusk.ProgramArgs(AnyPointer, AnyPointer));
}

interface OutputPortal extends(Dusk.Portal) {
  output @0 (stream :Dusk.Stream) -> (daemonize :Bool);
}

interface ShStop {
  stop @0 () -> ();
}

interface ShPortal extends(Dusk.Portal, OutputPortal) {
  sh @0 (script :Bytecode.Script, output :Dusk.Stream, stop :ShStop, compiler :Compiler) -> ();
  functions @1 () -> (symbols :List(Text));
}
