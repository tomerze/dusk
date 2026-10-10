@0xab8bd1aea40a7c58;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0xa491d262995861be;
const defaultPid :UInt64 = 0x88f2d85773361325;

const scanTypeId :UInt64 = 0x84e09148e9d394f3;

# key is fnv1a of the key name, salted as `dusk_program_kvs_internal::key_id`
# does it.
struct KvsArgs {
  struct Data {
    union {
      get @0 :List(UInt64);
      set :group {
        key @1 :UInt64;
        value @2 :Dusk.Value;
        flags @8 :UInt8;
      }
      delete @3 :UInt64;
      exists @4 :UInt64;
      server @5 :Void;
      scan @6 :Void;
      bind @9 :Text;
    }
    forbiddenUnstick @7 :Bool;
  }
  interface Server {
    transpose @0 (keys :List(UInt64), output :Dusk.Stream, values :List(Dusk.Value), flags :List(UInt8)) -> ();
  }
}

interface KvsPortal extends(Dusk.Portal, Sh.OutputPortal) {
  get @0 (key :UInt64) -> (value :Dusk.Value, flags :UInt8);
  set @1 (key :UInt64, value :Dusk.Value, forbiddenUnstick :Bool, flags :UInt8) -> ();
  delete @2 (key :UInt64, forbiddenUnstick :Bool) -> (deleted :Bool);
  exists @3 (key :UInt64) -> (exists :Bool);
  scan @4 (output :Dusk.Stream) -> ();
}
