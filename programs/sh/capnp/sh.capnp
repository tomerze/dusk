@0xb25a041190c0e845;

using Dusk = import "/capnp/dusk.capnp";

const programId :UInt64 = 0x8d0e0504ec994ea4;

interface Engine {
  buildProgramArgsFromString @0 (string :Text) -> (program_args: Dusk.ProgramArgs);
  client @1 () -> (client :Dusk.Dusk);
}

interface ShArgs extends(Dusk.ProgramArgs) {
  get @0 () -> (engine :Engine);
}

interface ShPortal extends(Dusk.Portal) {
  sh @0 (command :Text, output :Dusk.Stream) -> ();
}

interface OutputPortal extends(Dusk.Portal) {
  output @0 (stream :Dusk.Stream) -> ();
}
