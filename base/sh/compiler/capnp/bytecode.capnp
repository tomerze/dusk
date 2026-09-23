@0xf5f34f381cd409b5;

struct Script {
  struct Statement {
    struct Expr {
      struct ExprPair {
        first @0: Expr;
        second @1: Expr;
      }
      union {
        command @0: Text;
        and @1: ExprPair;
        or @2: ExprPair;
      }
    }
    struct FunctionDefinition {
      symbol @0: Text;
      body @1: Script;
    }
    union {
      expr @0: Expr;
      functionDefinition @1: FunctionDefinition;
    }
  }
  statements @0 :List(Statement);
}
