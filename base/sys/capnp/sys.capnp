@0xfcfb214a95217a0a;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0x8253da246ad23249;

struct SysArgs {
  struct Data {
    command @0 :Text;
  }
  interface Server {}
}

interface SysPortal extends(Dusk.Portal, Sh.OutputPortal) {}
