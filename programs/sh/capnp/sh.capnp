@0xb25a041190c0e845;

using Dusk = import "/capnp/dusk.capnp";

const programId :UInt64 = 0x8d0e0504ec994ea4;

interface Engine {
  buildProgramArgsFromString @0 (string :Text) -> (program_args: Dusk.ProgramArgs);
  dissectOutput @1 (program_args: Dusk.ProgramArgs, output :Dusk.Stream) -> (output :Dusk.ProgramArgs);
}

interface ShArgs extends(Dusk.ProgramArgs) {
  get @0 () -> (engine :Engine, client :Dusk.Dusk);
}

interface ShPortal extends(Dusk.Portal) {
  sh @0 (command :Text, output :Dusk.Stream) -> ();
}
