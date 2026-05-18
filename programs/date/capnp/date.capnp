@0xdf7b3ef07aaf5aa2;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0xa47a3ddd6d1d9fe5;

struct DateArgs {
  struct Data {
    union {
      show @0 :Void;
      setTo @1 :UInt64;  # unix_time_ms
    }
  }
  interface Server {}
}

interface DatePortal extends(Dusk.Portal, Sh.OutputPortal) {}
