use std::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub struct Ast<'a> {
    pub statements: Vec<Statement<'a>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Statement<'a> {
    pub expr: Expr<'a>,
    pub background: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr<'a> {
    Command(Command<'a>),
    And(Box<Expr<'a>>, Box<Expr<'a>>),
    Or(Box<Expr<'a>>, Box<Expr<'a>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Command<'a> {
    pub program: &'a str,
    pub args: Vec<&'a str>,
}
