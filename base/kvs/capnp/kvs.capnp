@0xab8bd1aea40a7c58;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0xa491d262995861be;

struct KvsArgs {
  struct Data {}
  interface Server {}
}

interface KvsPortal extends(Dusk.Portal, Sh.OutputPortal) {}
