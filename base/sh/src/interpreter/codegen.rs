use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use core::cell::RefCell;
use core::future::Future;
use core::pin::Pin;
use dusk_program::anyhow::{Result, anyhow};
use dusk_program::program_args::ProgramArgs;
use hashbrown::HashSet;

use crate::bytecode;

use super::instructions::{Inst, Instructions};
use super::{FunctionTable, GeneratedFunctions};
use crate::BytecodeMessage;
use dusk_capnp::capnp::message::HeapAllocator;

pub(super) fn generate(script: bytecode::Reader<'_>) -> Result<Instructions> {
    let mut output_instructions = Instructions::new();
    generate_script(script, &mut output_instructions)?;
    optimize_tail_call(&mut output_instructions);
    Ok(output_instructions)
}

pub(super) async fn generate_function(
    function_table: &FunctionTable,
    generated_functions: &GeneratedFunctions,
    symbol: &str,
) -> Result<Arc<Instructions>> {
    generate_function_once(
        function_table,
        generated_functions,
        symbol,
        &mut HashSet::new(),
    )
    .await
}

#[allow(clippy::arc_with_non_send_sync)]
fn generate_function_once<'a>(
    function_table: &'a FunctionTable,
    generated_functions: &'a GeneratedFunctions,
    symbol: &'a str,
    generating: &'a mut HashSet<String>,
) -> Pin<Box<dyn Future<Output = Result<Arc<Instructions>>> + 'a>> {
    Box::pin(async move {
        if let Some(instructions) = generated_functions.borrow().get(symbol).cloned() {
            return Ok(instructions);
        }
        if !generating.insert(symbol.to_string()) {
            return Ok(Arc::new(Instructions::new()));
        }
        let body = function_table
            .lock()
            .await
            .get(symbol)
            .cloned()
            .ok_or_else(|| anyhow!("call to unknown symbol: {}", symbol))?;

        let instructions = generate(
            body.borrow_mut()
                .get_root::<bytecode::Builder>()?
                .into_reader(),
        )?;
        tracing::debug!(
            dump = %super::instructions::format_instructions(&instructions),
            symbol = symbol,
            "function instruction disassembly"
        );

        // Eagerly compile every function this body calls.
        let dep_symbols: alloc::vec::Vec<String> = instructions
            .iter()
            .filter_map(|i| match i {
                Inst::Call(s) | Inst::TailCall(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        for dep in dep_symbols {
            // Ignore errors / missing bodies - runtime resolve will surface them.
            let _ =
                generate_function_once(function_table, generated_functions, &dep, generating).await;
        }

        let instructions = Arc::new(instructions);
        let still_defined = function_table
            .lock()
            .await
            .get(symbol)
            .is_some_and(|current| Rc::ptr_eq(current, &body));
        if still_defined {
            generated_functions
                .borrow_mut()
                .insert(symbol.to_string(), instructions.clone());
        }
        Ok(instructions)
    })
}

fn optimize_tail_call(instructions: &mut Instructions) {
    if let Some(last) = instructions.last_mut()
        && let Inst::Call(symbol) = last
    {
        let symbol = core::mem::take(symbol);
        *last = Inst::TailCall(symbol);
    }
}

fn generate_script(
    script: bytecode::Reader<'_>,
    output_instructions: &mut Instructions,
) -> Result<()> {
    for statement in script.get_statements()?.iter() {
        generate_statement(statement, output_instructions)?;
    }
    Ok(())
}

#[allow(clippy::arc_with_non_send_sync)]
fn generate_statement(
    statement: bytecode::statement::Reader<'_>,
    output_instructions: &mut Instructions,
) -> Result<()> {
    use bytecode::statement::Which;
    match statement.which()? {
        Which::Expr(expr) => generate_expr(expr?, output_instructions),
        Which::FunctionDefinition(def) => {
            let def = def?;
            let symbol = def.get_symbol()?.to_str()?.to_string();
            output_instructions.push(Inst::DefineFunction {
                symbol,
                body: {
                    let mut body = BytecodeMessage::new(HeapAllocator::new());
                    body.set_root::<bytecode::Owned>(def.get_body()?)?;
                    Rc::new(RefCell::new(body))
                },
            });
            Ok(())
        }
    }
}

fn generate_expr(
    expr: bytecode::statement::expr::Reader<'_>,
    output_instructions: &mut Instructions,
) -> Result<()> {
    use bytecode::statement::expr::Which;
    match expr.which()? {
        Which::ProgramArgs(Ok(program_args)) => {
            output_instructions.push(Inst::ProgramArgs(ProgramArgs::from_reader(program_args)?));
            Ok(())
        }
        Which::Call(Ok(symbol)) => {
            output_instructions.push(Inst::Call(symbol.to_str()?.to_string()));
            Ok(())
        }
        Which::And(Ok(pair)) => {
            generate_expr(pair.reborrow().get_first()?, output_instructions)?;
            let jump_idx = output_instructions.len();
            output_instructions.push(Inst::JumpIfError(0)); // patched below
            generate_expr(pair.reborrow().get_second()?, output_instructions)?;
            let target = output_instructions.len();
            output_instructions[jump_idx] = Inst::JumpIfError(target);
            Ok(())
        }
        Which::Or(Ok(pair)) => {
            generate_expr(pair.reborrow().get_first()?, output_instructions)?;
            let jump_idx = output_instructions.len();
            output_instructions.push(Inst::JumpIfOk(0)); // patched below
            generate_expr(pair.reborrow().get_second()?, output_instructions)?;
            let target = output_instructions.len();
            output_instructions[jump_idx] = Inst::JumpIfOk(target);
            Ok(())
        }
        _ => Err(anyhow!("error generating an expression")),
    }
}
