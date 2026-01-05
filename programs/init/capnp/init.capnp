@0xc2255dd10715a1d5;

using Dusk = import "/capnp/dusk.capnp";

const programId :UInt64 = 0xd77c7f8193a1856c;

interface InitArgs extends(Dusk.ProgramArgs) {}

interface InitPortal extends(Dusk.Portal) {}
