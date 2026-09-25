use alloc::string::{String, ToString};
use alloc::sync::Arc;
use core::future::Future;
use core::pin::Pin;
use dusk_program::anyhow::{Result, anyhow};
use hashbrown::HashSet;

use crate::script;
use crate::sh_capnp;

use super::instructions::{self, Frame, Inst, ScriptWrapper};
use super::{CompiledFunctions, FunctionTable};

pub(super) async fn codegen(
    script: script::Reader<'_>,
    compiler: sh_capnp::compiler::Client,
    mut functions: HashSet<String>,
) -> Result<Frame> {
    let mut output_frame = Frame::new();
    codegen_script(script, &compiler, &mut functions, &mut output_frame).await?;
    optimize_tail_call(&mut output_frame);
    Ok(output_frame)
}

pub(super) async fn codegen_function(
    function_table: &FunctionTable,
    compiled_functions: &CompiledFunctions,
    compiler: sh_capnp::compiler::Client,
    symbol: &str,
) -> Result<Arc<Frame>> {
    codegen_function_once(
        function_table,
        compiled_functions,
        compiler,
        symbol,
        &mut HashSet::new(),
    )
    .await
}

#[allow(clippy::arc_with_non_send_sync)]
fn codegen_function_once<'a>(
    function_table: &'a FunctionTable,
    compiled_functions: &'a CompiledFunctions,
    compiler: sh_capnp::compiler::Client,
    symbol: &'a str,
    compiling: &'a mut HashSet<String>,
) -> Pin<Box<dyn Future<Output = Result<Arc<Frame>>> + 'a>> {
    Box::pin(async move {
        if let Some(frame) = compiled_functions.borrow().get(symbol).cloned() {
            return Ok(frame);
        }
        if !compiling.insert(symbol.to_string()) {
            return Ok(Arc::new(Frame::new()));
        }
        let body = function_table
            .lock()
            .await
            .get(symbol)
            .cloned()
            .ok_or_else(|| anyhow!("unknown function: {}", symbol))?;

        let symbols: HashSet<String> = function_table.lock().await.keys().cloned().collect();
        let script = body.0.get_root_as_reader::<script::Reader<'_>>()?;
        let frame = codegen(script, compiler.clone(), symbols).await?;
        tracing::debug!(
            dump = %instructions::format_instructions(&frame),
            symbol = symbol,
            "function frame disassembly"
        );

        // Eagerly compile every function this body calls.
        let dep_symbols: alloc::vec::Vec<String> = frame
            .iter()
            .filter_map(|i| match i {
                Inst::Call(s) | Inst::TailCall(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        for dep in dep_symbols {
            // Ignore errors / missing bodies - runtime resolve will surface them.
            let _ = codegen_function_once(
                function_table,
                compiled_functions,
                compiler.clone(),
                &dep,
                compiling,
            )
            .await;
        }

        let frame = Arc::new(frame);
        let still_defined = function_table
            .lock()
            .await
            .get(symbol)
            .is_some_and(|current| Arc::ptr_eq(current, &body));
        if still_defined {
            compiled_functions
                .borrow_mut()
                .insert(symbol.to_string(), frame.clone());
        }
        Ok(frame)
    })
}

fn optimize_tail_call(frame: &mut Frame) {
    if let Some(last) = frame.last_mut()
        && let Inst::Call(symbol) = last
    {
        let symbol = core::mem::take(symbol);
        *last = Inst::TailCall(symbol);
    }
}

fn codegen_script<'a>(
    script: script::Reader<'a>,
    compiler: &'a sh_capnp::compiler::Client,
    functions: &'a mut HashSet<String>,
    output_frame: &'a mut Frame,
) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
    Box::pin(async move {
        for statement in script.get_statements()?.iter() {
            codegen_statement(statement, compiler, functions, output_frame).await?;
        }
        Ok(())
    })
}

async fn codegen_statement<'a>(
    statement: script::statement::Reader<'a>,
    compiler: &sh_capnp::compiler::Client,
    functions: &mut HashSet<String>,
    output_frame: &mut Frame,
) -> Result<()> {
    use script::statement::Which;
    match statement.which()? {
        Which::Expr(expr) => codegen_expr(expr?, compiler, functions, output_frame).await,
        Which::FunctionDefinition(def) => {
            let def = def?;
            let symbol = def.get_symbol()?.to_str()?.to_string();
            functions.insert(symbol.clone());
            let mut message = capnp::message::Builder::new(capnp::message::HeapAllocator::new());
            message.set_root(def.get_body()?)?;
            output_frame.push(Inst::DefineFunction {
                symbol,
                body: Arc::new(ScriptWrapper(message)),
            });
            Ok(())
        }
    }
}

fn codegen_expr<'a>(
    expr: script::statement::expr::Reader<'a>,
    compiler: &'a sh_capnp::compiler::Client,
    functions: &'a mut HashSet<String>,
    output_frame: &'a mut Frame,
) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
    Box::pin(async move {
        use script::statement::expr::Which;
        match expr.which()? {
            Which::Command(Ok(text)) => {
                let text = text.to_str()?;
                let first_word = text.split_whitespace().next().unwrap_or("");
                if functions.contains(first_word) {
                    if text.split_whitespace().nth(1).is_some() {
                        return Err(anyhow!("function `{}` cannot take arguments", first_word));
                    }
                    output_frame.push(Inst::Call(first_word.to_string()));
                } else {
                    let mut request = compiler.build_program_args_request();
                    request.get().set_command(text);
                    let reply = request.send().promise.await?;
                    let program_args = dusk_program::program_args::ProgramArgs::from_reader(
                        reply.get()?.get_program_args()?,
                    )?;
                    output_frame.push(Inst::ProgramArgs(program_args));
                }
                Ok(())
            }
            Which::And(Ok(pair)) => {
                codegen_expr(
                    pair.reborrow().get_first()?,
                    compiler,
                    functions,
                    output_frame,
                )
                .await?;
                let jump_idx = output_frame.len();
                output_frame.push(Inst::JumpIfError(0)); // patched below
                codegen_expr(
                    pair.reborrow().get_second()?,
                    compiler,
                    functions,
                    output_frame,
                )
                .await?;
                let target = output_frame.len();
                output_frame[jump_idx] = Inst::JumpIfError(target);
                Ok(())
            }
            Which::Or(Ok(pair)) => {
                codegen_expr(
                    pair.reborrow().get_first()?,
                    compiler,
                    functions,
                    output_frame,
                )
                .await?;
                let jump_idx = output_frame.len();
                output_frame.push(Inst::JumpIfOk(0)); // patched below
                codegen_expr(
                    pair.reborrow().get_second()?,
                    compiler,
                    functions,
                    output_frame,
                )
                .await?;
                let target = output_frame.len();
                output_frame[jump_idx] = Inst::JumpIfOk(target);
                Ok(())
            }
            _ => Err(anyhow!("error compiling expression")),
        }
    })
}
