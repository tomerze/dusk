@0xf059f27afd7e0035;

using Dusk = import "/capnp/dusk.capnp";

const programId :UInt64 = 0xd111e8c31818511d;

const resultTypeId :UInt64 = 0xcef2c7c974bf44ec;

struct PsOptions {}

interface PsArgs extends(Dusk.ProgramArgs) {
  get @0 () -> (client: Dusk.Dusk, options :PsOptions);
}

interface PsPortal extends(Dusk.Portal) {
  sh @0 (command :Text, output :Dusk.Stream) -> (input :Dusk.Stream);
}
