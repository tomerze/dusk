@0xf059f27afd7e0035;

using Dusk = import "/capnp/dusk.capnp";

const programId :UInt64 = 0xd111e8c31818511d;

struct PsOptions {
    pids @0 :List(UInt64);
    programIds @1 :List(UInt64);
}

interface PsArgs extends(Dusk.ProgramArgs) {
  get @0 () -> (client: Dusk.Dusk, options :PsOptions);
}

interface PsPortal extends(Dusk.Portal) {
  sh @0 (command :Text, output :Dusk.Stream) -> (input :Dusk.Stream);
}
