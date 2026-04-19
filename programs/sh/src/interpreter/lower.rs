use alloc::vec::Vec;
use dusk_capnp::dusk_capnp::stream;
use dusk_program::anyhow::{Result, anyhow};

use crate::sh_capnp::script;

use super::inst::Inst;

pub fn lower(script: script::Reader<'_>, output: &stream::Client) -> Result<Vec<Inst>> {
    let mut instructions = Vec::new();
    let statements = script.get_statements()?;

    for statement in statements.iter() {
        lower_expr(statement.get_expr()?, output, &mut instructions)?;
    }

    Ok(instructions)
}

fn lower_expr(
    expr: script::statement::expr::Reader<'_>,
    output: &stream::Client,
    instructions: &mut Vec<Inst>,
) -> Result<()> {
    use script::statement::expr::Which;
    match expr.which()? {
        Which::ProgramArgs(Ok(pa)) => {
            instructions.push(Inst::Command {
                program_args: pa,
                output: output.clone(),
            });
            Ok(())
        }
        Which::And(Ok(pair)) => {
            lower_expr(pair.reborrow().get_first()?, output, instructions)?;
            let jump_idx = instructions.len();
            instructions.push(Inst::JumpIfError(0)); // patched below
            lower_expr(pair.reborrow().get_second()?, output, instructions)?;
            let target = instructions.len();
            instructions[jump_idx] = Inst::JumpIfError(target);
            Ok(())
        }
        Which::Or(Ok(pair)) => {
            lower_expr(pair.reborrow().get_first()?, output, instructions)?;
            let jump_idx = instructions.len();
            instructions.push(Inst::JumpIfOk(0)); // patched below
            lower_expr(pair.reborrow().get_second()?, output, instructions)?;
            let target = instructions.len();
            instructions[jump_idx] = Inst::JumpIfOk(target);
            Ok(())
        }
        _ => Err(anyhow!("error lowering expression")),
    }
}
