@0xc2255dd10715a1d5;

using Dusk = import "/capnp/dusk.capnp";
using Compiler = import "/capnp/bytecode.capnp";

const programId :UInt64 = 0xd77c7f8193a1856c;

struct InitArgs {
  struct Data {
    initScript @0 :Compiler.Bytecode;
  }
  interface Server {}
}

interface InitPortal extends(Dusk.Portal) {}
