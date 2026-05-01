use alloc::rc::Rc;
use alloc::string::ToString;
use alloc::vec::Vec;
use dusk_program::anyhow::{Result, anyhow};

use crate::sh_capnp::script;

use super::inst::Inst;

pub fn lower(script: script::Reader<'_>) -> Result<Vec<Inst>> {
    let mut instructions = Vec::new();
    let statements = script.get_statements()?;

    for statement in statements.iter() {
        lower_statement(statement, &mut instructions)?;
    }

    optimize_tail_call(&mut instructions);
    Ok(instructions)
}

fn optimize_tail_call(instructions: &mut [Inst]) {
    if let Some(last) = instructions.last_mut()
        && let Inst::Call(name) = last
    {
        let name = core::mem::take(name);
        *last = Inst::TailCall(name);
    }
}

fn lower_statement(
    statement: script::statement::Reader<'_>,
    instructions: &mut Vec<Inst>,
) -> Result<()> {
    use script::statement::Which;
    match statement.which()? {
        Which::Expr(expr) => lower_expr(expr?, instructions),
        Which::FunctionDefinition(def) => {
            let def = def?;
            let name = def.get_name()?.to_str()?.to_string();
            let frame = Rc::new(lower(def.get_body()?)?);
            instructions.push(Inst::DefineFunction { name, frame });
            Ok(())
        }
    }
}

fn lower_expr(
    expr: script::statement::expr::Reader<'_>,
    instructions: &mut Vec<Inst>,
) -> Result<()> {
    use script::statement::expr::Which;
    match expr.which()? {
        Which::ProgramArgs(Ok(pa)) => {
            instructions.push(Inst::Command { program_args: pa });
            Ok(())
        }
        Which::Call(Ok(name)) => {
            let name = name.to_str()?.to_string();
            instructions.push(Inst::Call(name));
            Ok(())
        }
        Which::TailCall(Ok(name)) => {
            let name = name.to_str()?.to_string();
            instructions.push(Inst::TailCall(name));
            Ok(())
        }
        Which::And(Ok(pair)) => {
            lower_expr(pair.reborrow().get_first()?, instructions)?;
            let jump_idx = instructions.len();
            instructions.push(Inst::JumpIfError(0)); // patched below
            lower_expr(pair.reborrow().get_second()?, instructions)?;
            let target = instructions.len();
            instructions[jump_idx] = Inst::JumpIfError(target);
            Ok(())
        }
        Which::Or(Ok(pair)) => {
            lower_expr(pair.reborrow().get_first()?, instructions)?;
            let jump_idx = instructions.len();
            instructions.push(Inst::JumpIfOk(0)); // patched below
            lower_expr(pair.reborrow().get_second()?, instructions)?;
            let target = instructions.len();
            instructions[jump_idx] = Inst::JumpIfOk(target);
            Ok(())
        }
        _ => Err(anyhow!("error lowering expression")),
    }
}
