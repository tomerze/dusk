@0xbbc8950943b947bc;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0xc0da1e11ed8d4a36;

struct SleepArgs {
  struct Data {
    durationMs @0 :UInt64;
  }
  interface Server {}
}

interface SleepPortal extends(Dusk.Portal, Sh.OutputPortal) {}
