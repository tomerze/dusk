@0xc2255dd10715a1d5;

using Dusk = import "/capnp/dusk.capnp";

const programId :UInt64 = 0xd77c7f8193a1856c;

struct InitArgs {
  struct Data {
    address @0 :Text;
    port @1 :UInt16;
  }
  interface Server {}
}

interface InitPortal extends(Dusk.Portal) {}
