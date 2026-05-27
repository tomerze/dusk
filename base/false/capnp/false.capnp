@0xc20686eb11a50206;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0xf06ab032aedb21a5;

struct FalseArgs {
  struct Data {}
  interface Server {}
}

interface FalsePortal extends(Dusk.Portal, Sh.OutputPortal) {}
