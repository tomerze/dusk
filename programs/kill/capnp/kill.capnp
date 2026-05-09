@0x8c8ca34b18878fbe;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0x9e2a072da809eefe;

struct KillArgs {
  struct Data {
    pid    @0 :UInt64;
    signal @1 :UInt64;
  }
  interface Server {}
}

interface KillPortal extends(Dusk.Portal, Sh.OutputPortal) {}
