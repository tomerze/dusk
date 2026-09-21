use crate::anyhow::Result;
use dusk_program::anyhow;
use dusk_program_sh_bytecode::{ast, tokenize};

pub use dusk_program_sh_bytecode::tokenize::{command_words, strip_comments};

#[derive(Clone, Default)]
pub struct Parser;

impl Parser {
    pub fn new() -> Self {
        Self
    }

    fn parse_expr_pair(
        &mut self,
        first: &ast::Expr,
        second: &ast::Expr,
        builder: &mut crate::sh_capnp::script::statement::expr::expr_pair::Builder,
    ) -> Result<()> {
        let mut first_builder = builder.reborrow().init_first();
        self.parse_expr(first, &mut first_builder)?;
        let mut second_builder = builder.reborrow().init_second();
        self.parse_expr(second, &mut second_builder)?;
        Ok(())
    }

    fn parse_expr(
        &mut self,
        expr: &ast::Expr,
        builder: &mut crate::sh_capnp::script::statement::expr::Builder,
    ) -> Result<()> {
        match expr {
            ast::Expr::Command(source) => {
                builder.reborrow().set_command(*source);
            }
            ast::Expr::And(first, second) => {
                let mut and_builder = builder.reborrow().init_and();
                self.parse_expr_pair(first, second, &mut and_builder)?;
            }
            ast::Expr::Or(first, second) => {
                let mut or_builder = builder.reborrow().init_or();
                self.parse_expr_pair(first, second, &mut or_builder)?;
            }
        }
        Ok(())
    }

    fn parse_statement(
        &mut self,
        stmt: &ast::Statement,
        builder: &mut crate::sh_capnp::script::statement::Builder,
    ) -> Result<()> {
        match stmt {
            ast::Statement::Expr(expr) => {
                let mut expr_builder = builder.reborrow().init_expr();
                self.parse_expr(expr, &mut expr_builder)?;
            }
            ast::Statement::FunctionDefinition { symbol, body } => {
                let mut def_builder = builder.reborrow().init_function_definition();
                def_builder.set_symbol(*symbol);
                let body_builder = def_builder.init_body();
                self.parse_ast(body, body_builder)?;
            }
        }
        Ok(())
    }

    fn parse_ast(
        &mut self,
        program_ast: &ast::Ast,
        builder: crate::sh_capnp::script::Builder,
    ) -> Result<()> {
        let mut stmts = builder.init_statements(program_ast.statements.len() as u32);
        for (i, ast_stmt) in program_ast.statements.iter().enumerate() {
            let mut stmt = stmts.reborrow().get(i as u32);
            self.parse_statement(ast_stmt, &mut stmt)?;
        }
        Ok(())
    }

    pub fn parse(&mut self, s: &str, builder: crate::sh_capnp::script::Builder) -> Result<()> {
        let stripped = tokenize::strip_comments(s);
        let (remaining, program_ast) =
            tokenize::ast(&stripped).map_err(|_| anyhow::anyhow!("syntax error"))?;
        if !remaining.trim().is_empty() {
            return Err(anyhow::anyhow!("syntax error"));
        }

        self.parse_ast(&program_ast, builder)
    }
}
