@0xc84b54843dccc76d;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0xa5f2d74133afb928;

const resultTypeId :UInt64 = 0xd59cc343d54ebec0;

struct ProgramsArgs {
  struct Data {}
  interface Server {
    transpose @0 (programIds :List(UInt64),
                  versions :List(Text),
                  gitRevisions :List(Text),
                  output :Dusk.Stream) -> ();
  }
}

interface ProgramsPortal extends(Dusk.Portal, Sh.OutputPortal) {}
