@0xb25a041190c0e845;

using Dusk = import "/capnp/dusk.capnp";

const programId :UInt64 = 0x8d0e0504ec994ea4;

struct Script {
  struct Statement {
    struct Expr {
      struct ExprPair {
        first @0: Expr;
        second @1: Expr;
      }
      union {
        programArgs @0: Dusk.ProgramArgs;
        and @1: ExprPair;
        or @2: ExprPair;
      }
    }
    expr @0: Expr;
  }
  statements @0 :List(Statement);
}

struct ShOptions {
  union {
    server @0: Void;
    script @1: Script;
    detachedScript @2: Script;
  }
}

interface ShArgs extends(Dusk.ProgramArgs) {
  get @0 () -> (client: Dusk.Dusk, options :ShOptions);
}

interface OutputPortal extends(Dusk.Portal) {
  output @0 (stream :Dusk.Stream) -> ();
}

interface ShPortal extends(Dusk.Portal, OutputPortal) {
  sh @0 (script :Script, output :Dusk.Stream) -> ();
}
