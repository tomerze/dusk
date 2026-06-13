@0x9e48c684d8c17a73;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0xf471a505ba1b8328;

struct EchoArgs {
  struct Data {
    text @0 :Text;
  }
  interface Server {}
}

interface EchoPortal extends(Dusk.Portal, Sh.OutputPortal) {}
