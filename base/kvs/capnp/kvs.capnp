@0xab8bd1aea40a7c58;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0xa491d262995861be;

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
      }
      delete @3 :UInt64;
      exists @4 :UInt64;
      bind @5 :Void;
      scan @6 :Void;
    }
    forbiddenUnstick @7 :Bool;
  }
  interface Server {
    transpose @0 (keys :List(UInt64), output :Dusk.Stream, values :List(Dusk.Value)) -> ();
  }
}

interface KvsPortal extends(Dusk.Portal, Sh.OutputPortal) {
  get @0 (key :UInt64) -> (value :Dusk.Value);
  set @1 (key :UInt64, value :Dusk.Value, forbiddenUnstick :Bool) -> ();
  delete @2 (key :UInt64, forbiddenUnstick :Bool) -> (deleted :Bool);
  exists @3 (key :UInt64) -> (exists :Bool);
  scan @4 (output :Dusk.Stream) -> ();
}
