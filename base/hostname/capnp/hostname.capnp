@0x93667ea94003a85e;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0xd80c8029c1df7490;

struct HostnameArgs {
  struct Data {}
  interface Server {}
}

interface HostnamePortal extends(Dusk.Portal, Sh.OutputPortal) {}
