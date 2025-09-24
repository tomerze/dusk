@0xb25a041190c0e845;

using Dusk = import "/capnp/dusk.capnp";

const programId :UInt64 = 0x8d0e0504ec994ea4;

interface ProgramArgsBuilder {
  buildFromString @0 (string :Text) -> (result: Dusk.ProgramArgs);
}

interface ShArgs extends(Dusk.ProgramArgs) {
  get @0 () -> (program_args_builder :ProgramArgsBuilder, client :Dusk.Dusk);
}

interface ShPortal extends(Dusk.Portal) {
  sh @0 (command :Text, output :Dusk.Stream) -> (input :Dusk.Stream);
}
