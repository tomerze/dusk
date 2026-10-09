@0xe3ccadf5d52d17fa;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0xd089c575e560637e;

struct NightfallArgs {
  struct Data {
    address @0 :Text;
    port @1 :UInt16;
    connect @2 :Connect;
  }
  struct Connect {
    fleet @0 :Text;
    fleetServerName @1 :Text;
    provision @2 :Text;
    provisionServerName @3 :Text;
    trustAnchors @4 :Text;
    installTokenFile @5 :Text;
    heartbeatTimeoutSeconds @6 :UInt32 = 90;
  }
  interface Server {}
}

interface NightfallPortal extends(Dusk.Portal, Sh.OutputPortal) {}
