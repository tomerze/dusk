@0xb25a041190c0e845;

using Dusk = import "/capnp/dusk.capnp";

const programId :UInt64 = 0x8d0e0504ec994ea4;

interface ShArgs extends(Dusk.ProgramArgs) {}

interface ShPortal extends(Dusk.Portal) {
  sh @0 (command :Text, output :Dusk.Stream) -> (input :Dusk.Stream);  
  getEnv @1 (key :Data) -> (value :AnyPointer);
  setEnv @2 (key :Data, value :AnyPointer) -> ();
}
