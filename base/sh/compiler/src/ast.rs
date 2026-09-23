use alloc::boxed::Box;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub struct Ast<'a> {
    pub statements: Vec<Statement<'a>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement<'a> {
    Expr(Expr<'a>),
    FunctionDefinition { symbol: &'a str, body: Ast<'a> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr<'a> {
    Command(&'a str),
    And(Box<Expr<'a>>, Box<Expr<'a>>),
    Or(Box<Expr<'a>>, Box<Expr<'a>>),
}
