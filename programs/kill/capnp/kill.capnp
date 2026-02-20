@0x8c8ca34b18878fbe;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0x9e2a072da809eefe;

const resultTypeId :UInt64 = 0xe67b052cb8b1db49;

struct KillOptions {
  pid @0: UInt64;
  signal @1: UInt64;
}

interface KillArgs extends(Dusk.ProgramArgs) {
  get @0 () -> (client: Dusk.Dusk, options :KillOptions);
}

interface KillPortal extends(Dusk.Portal, Sh.OutputPortal) {}
