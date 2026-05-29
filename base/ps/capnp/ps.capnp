@0xf059f27afd7e0035;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0xd111e8c31818511d;

const resultTypeId :UInt64 = 0xcef2c7c974bf44ec;

struct PsArgs {
  struct Data {
    union {
      all @0 :Void;
      pid @1 :UInt64;
    }
  }
  interface Server {}
}

interface PsPortal extends(Dusk.Portal, Sh.OutputPortal) {}
