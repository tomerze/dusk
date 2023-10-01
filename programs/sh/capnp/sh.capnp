@0xb25a041190c0e845;

using Dusk = import "/capnp/dusk.capnp";

const programId :UInt64 = 0x8d0e0504ec994ea4;

interface ShArgs extends(Dusk.ProgramArgs) {
  struct ProgramId {
    programId @0 :UInt64 = .programId;
  }
}


interface ShPortal extends(Dusk.Portal) {
  interface Stream {
    sendChunk @0 (chunk :Data) -> stream;
    done @1 () -> ();
  }
  sh @0 (command :Text, output :Stream) -> (input :Stream);  
  getEnv @1 (key :Data) -> (value :AnyPointer);
  setEnv @2 (key :Data, value :AnyPointer) -> ();
}
