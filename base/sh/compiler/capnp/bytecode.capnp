@0xf5f34f381cd409b5;

using Dusk = import "/capnp/dusk.capnp";

struct Bytecode {
  struct Statement {
    struct Expr {
      struct ExprPair {
        first @0: Expr;
        second @1: Expr;
      }
      union {
        programArgs @0: Dusk.ProgramArgs(AnyPointer, AnyPointer);
        and @1: ExprPair;
        or @2: ExprPair;
        call @3: Text;
      }
    }
    struct FunctionDefinition {
      symbol @0: Text;
      body @1: Bytecode;
    }
    union {
      expr @0: Expr;
      functionDefinition @1: FunctionDefinition;
    }
  }
  statements @0 :List(Statement);
}
