use crate::anyhow::Result;
use dusk_program::anyhow;

mod ast;
mod parse;

pub use parse::command_words;

#[derive(Clone, Default)]
pub struct Compiler;

impl Compiler {
    pub fn new() -> Self {
        Self
    }

    fn compile_expr_pair(
        &mut self,
        first: &ast::Expr,
        second: &ast::Expr,
        builder: &mut crate::sh_capnp::script::statement::expr::expr_pair::Builder,
    ) -> Result<()> {
        let mut first_builder = builder.reborrow().init_first();
        self.compile_expr(first, &mut first_builder)?;
        let mut second_builder = builder.reborrow().init_second();
        self.compile_expr(second, &mut second_builder)?;
        Ok(())
    }

    fn compile_expr(
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
                self.compile_expr_pair(first, second, &mut and_builder)?;
            }
            ast::Expr::Or(first, second) => {
                let mut or_builder = builder.reborrow().init_or();
                self.compile_expr_pair(first, second, &mut or_builder)?;
            }
        }
        Ok(())
    }

    fn compile_statement(
        &mut self,
        stmt: &ast::Statement,
        builder: &mut crate::sh_capnp::script::statement::Builder,
    ) -> Result<()> {
        match stmt {
            ast::Statement::Expr(expr) => {
                let mut expr_builder = builder.reborrow().init_expr();
                self.compile_expr(expr, &mut expr_builder)?;
            }
            ast::Statement::FunctionDefinition { name, body } => {
                let mut def_builder = builder.reborrow().init_function_definition();
                def_builder.set_name(*name);
                let body_builder = def_builder.init_body();
                self.compile_ast(body, body_builder)?;
            }
        }
        Ok(())
    }

    fn compile_ast(
        &mut self,
        program_ast: &ast::Ast,
        builder: crate::sh_capnp::script::Builder,
    ) -> Result<()> {
        let mut stmts = builder.init_statements(program_ast.statements.len() as u32);
        for (i, ast_stmt) in program_ast.statements.iter().enumerate() {
            let mut stmt = stmts.reborrow().get(i as u32);
            self.compile_statement(ast_stmt, &mut stmt)?;
        }
        Ok(())
    }

    pub fn compile(&mut self, s: &str, builder: crate::sh_capnp::script::Builder) -> Result<()> {
        let (remaining, program_ast) =
            parse::ast(s).map_err(|_| anyhow::anyhow!("syntax error"))?;
        if !remaining.trim().is_empty() {
            return Err(anyhow::anyhow!("syntax error"));
        }

        self.compile_ast(&program_ast, builder)
    }
}
