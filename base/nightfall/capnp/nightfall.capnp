@0xe3ccadf5d52d17fa;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0xd089c575e560637e;

struct NightfallArgs {
  struct Data {
    address @0 :Text;
    port @1 :UInt16;
  }
  interface Server {}
}

interface NightfallPortal extends(Dusk.Portal, Sh.OutputPortal) {}
