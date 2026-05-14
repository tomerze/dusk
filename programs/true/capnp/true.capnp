@0xc01e11b9769eaafa;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0x8fdaab33b38d2e8f;

struct TrueArgs {
  struct Data {}
  interface Server {}
}

interface TruePortal extends(Dusk.Portal, Sh.OutputPortal) {}
