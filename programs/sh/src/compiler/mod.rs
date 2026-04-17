use crate::anyhow::{Context, Result};
use crate::entry::ShEntriesBuilder;

use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::dusk_capnp::program_args;
use dusk_program::anyhow::anyhow;
use dusk_program::{IntoCapnp, anyhow};

mod ast;
mod parse;

#[derive(Clone)]
pub struct Compiler<S: ShEntriesBuilder> {
    client: dusk::Client,
    sh_entries_builder: S,
}

impl<S: ShEntriesBuilder> Compiler<S> {
    pub fn new(client: dusk::Client, sh_entries_builder: S) -> Self {
        Self {
            client,
            sh_entries_builder,
        }
    }

    /// Takes a string like `kill 1234` and returns a ProgramArgs ready to run.
    fn compile_program_args(&mut self, cmd: &ast::Command) -> Result<program_args::Client> {
        // split the string by the first space
        let sh_entries = self.sh_entries_builder.get_entries();

        for entry in sh_entries {
            if entry.info.name == cmd.program {
                let client = entry
                    .program_args_builder
                    .build(self.client.clone(), &cmd.args)
                    .context("program args builder failed")
                    .into_capnp()?;

                return Ok(client);
            }
        }
        Err(anyhow!("no sh entry found for `{}`", cmd.program))
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
            ast::Expr::Command(cmd) => {
                // Use your function to convert "cmd arg1 arg2" -> Dusk.ProgramArgs
                let program_args = self.compile_program_args(cmd)?;
                builder.reborrow().set_program_args(program_args);
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

    pub fn compile(&mut self, s: &str, builder: crate::sh_capnp::script::Builder) -> Result<()> {
        let (_, program_ast) = parse::ast(s).map_err(|_| anyhow::anyhow!("syntax error"))?;

        let mut stmts = builder.init_statements(program_ast.statements.len() as u32);
        for (i, ast_stmt) in program_ast.statements.into_iter().enumerate() {
            let mut stmt = stmts.reborrow().get(i as u32);
            stmt.set_background(ast_stmt.background);
            self.compile_expr(&ast_stmt.expr, &mut stmt.reborrow().init_expr())?;
        }

        Ok(())
    }
}
