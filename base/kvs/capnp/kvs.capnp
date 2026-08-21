@0xab8bd1aea40a7c58;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0xa491d262995861be;

struct KvsArgs {
  # `bind` has no KvsPortal counterpart: it is the absence of an operation.
  struct Data {
    union {
      get @0 :Text;
      set :group {
        key @1 :Text;
        value @2 :Dusk.Value;
      }
      delete @3 :Text;
      exists @4 :Text;
      bind @5 :Void;
    }
  }
  interface Server {}
}

interface KvsPortal extends(Dusk.Portal, Sh.OutputPortal) {
  get @0 (key :Text) -> (value :Dusk.Value);
  set @1 (key :Text, value :Dusk.Value) -> ();
  delete @2 (key :Text) -> (deleted :Bool);
  exists @3 (key :Text) -> (exists :Bool);
}
