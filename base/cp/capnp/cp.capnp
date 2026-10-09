@0x935cf23087407b27;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

const programId :UInt64 = 0x8779131bb51e0ca4;

const resultTypeId :UInt64 = 0xd82958034aade578;

using Bytes = Data;

interface Sink {
  write @0 (offset :UInt64, bytes :Data) -> stream;
  done @1 (end :UInt64) -> ();
}

struct Location {
  union {
    node @0 :Text;
    client @1 :Text;
  }
}

struct CpArgs {
  struct Data {
    source @0 :Location;
    destination @1 :Location;
  }
  interface Server {
    stat @0 (path :Text) -> (exists :Bool, length :UInt64);
    hash @1 (path :Text, length :UInt64) -> (hash :Bytes);
    read @2 (path :Text, offset :UInt64, end :UInt64, sink :Sink) -> ();
    write @3 (path :Text, offset :UInt64) -> (sink :Sink);
  }
}

interface CpPortal extends(Dusk.Portal, Sh.OutputPortal) {}
