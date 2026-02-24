@0xb25a041190c0e845;

using Dusk = import "/capnp/dusk.capnp";

const programId :UInt64 = 0x8d0e0504ec994ea4;

struct Script {
  programArgs @0: Dusk.ProgramArgs;
  background @1: Bool;
}

struct ShOptions {}

interface ShArgs extends(Dusk.ProgramArgs) {
  get @0 () -> (client: Dusk.Dusk, options :ShOptions);
}

interface ShPortal extends(Dusk.Portal) {
  sh @0 (script :Script, output :Dusk.Stream) -> ();
}

interface OutputPortal extends(Dusk.Portal) {
  output @0 (stream :Dusk.Stream) -> ();
}
