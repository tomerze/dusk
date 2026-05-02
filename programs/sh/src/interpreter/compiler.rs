use alloc::sync::Arc;
use alloc::string::{String, ToString};
use core::future::Future;
use core::pin::Pin;
use dusk_program::anyhow::{Result, anyhow};
use hashbrown::HashSet;

use crate::sh_capnp;
use crate::sh_capnp::script;

use super::{CompiledFunctions, FunctionTable};
use super::inst::{self, Frame, Inst, ScriptWrapper};

pub(super) async fn compile(
    script: script::Reader<'_>,
    sh_args: sh_capnp::sh_args::Client,
    mut functions: HashSet<String>,
) -> Result<Frame> {
    let mut output_frame = Frame::new();
    compile_script(script, &sh_args, &mut functions, &mut output_frame).await?;
    optimize_tail_call(&mut output_frame);
    Ok(output_frame)
}

pub(super) fn compile_function<'a>(
    function_table: &'a FunctionTable,
    compiled_functions: &'a CompiledFunctions,
    sh_args: sh_capnp::sh_args::Client,
    symbol: &'a str,
) -> Pin<Box<dyn Future<Output = Result<Arc<Frame>>> + 'a>> {
    Box::pin(async move {
        if let Some(frame) = compiled_functions.borrow().get(symbol).cloned() {
            return Ok(frame);
        }
        let body = function_table
            .lock()
            .await
            .get(symbol)
            .cloned()
            .ok_or_else(|| anyhow!("unknown function: {}", symbol))?;
        // Empty placeholder so recursive calls back to `symbol` short-circuit.
        compiled_functions
            .borrow_mut()
            .insert(symbol.to_string(), Arc::new(Frame::new()));

        let symbols: HashSet<String> = function_table.lock().await.keys().cloned().collect();
        let script = body.0.get_root_as_reader::<script::Reader<'_>>()?;
        let frame = match compile(script, sh_args.clone(), symbols).await {
            Ok(f) => f,
            Err(e) => {
                compiled_functions.borrow_mut().remove(symbol);
                return Err(e);
            }
        };
        tracing::debug!(
            dump = %inst::format_instructions(&frame),
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
            // Ignore errors / missing bodies — runtime resolve will surface them.
            let _ = compile_function(function_table, compiled_functions, sh_args.clone(), &dep)
                .await;
        }

        let frame = Arc::new(frame);
        compiled_functions
            .borrow_mut()
            .insert(symbol.to_string(), frame.clone());
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

fn compile_script<'a>(
    script: script::Reader<'a>,
    sh_args: &'a sh_capnp::sh_args::Client,
    functions: &'a mut HashSet<String>,
    output_frame: &'a mut Frame,
) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
    Box::pin(async move {
        for statement in script.get_statements()?.iter() {
            compile_statement(statement, sh_args, functions, output_frame).await?;
        }
        Ok(())
    })
}

async fn compile_statement<'a>(
    statement: script::statement::Reader<'a>,
    sh_args: &sh_capnp::sh_args::Client,
    functions: &mut HashSet<String>,
    output_frame: &mut Frame,
) -> Result<()> {
    use script::statement::Which;
    match statement.which()? {
        Which::Expr(expr) => compile_expr(expr?, sh_args, functions, output_frame).await,
        Which::FunctionDefinition(def) => {
            let def = def?;
            let symbol = def.get_symbol()?.to_str()?.to_string();
            functions.insert(symbol.clone());
            let mut message =
                capnp::message::Builder::new(capnp::message::HeapAllocator::new());
            message.set_root(def.get_body()?)?;
            output_frame.push(Inst::DefineFunction {
                symbol,
                body: Arc::new(ScriptWrapper(message)),
            });
            Ok(())
        }
    }
}

fn compile_expr<'a>(
    expr: script::statement::expr::Reader<'a>,
    sh_args: &'a sh_capnp::sh_args::Client,
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
                    let mut request = sh_args.build_program_args_request();
                    request.get().set_command(text);
                    let reply = request.send().promise.await?;
                    let program_args = reply.get()?.get_program_args()?;
                    output_frame.push(Inst::ProgramArgs(program_args));
                }
                Ok(())
            }
            Which::And(Ok(pair)) => {
                compile_expr(
                    pair.reborrow().get_first()?,
                    sh_args,
                    functions,
                    output_frame,
                )
                .await?;
                let jump_idx = output_frame.len();
                output_frame.push(Inst::JumpIfError(0)); // patched below
                compile_expr(
                    pair.reborrow().get_second()?,
                    sh_args,
                    functions,
                    output_frame,
                )
                .await?;
                let target = output_frame.len();
                output_frame[jump_idx] = Inst::JumpIfError(target);
                Ok(())
            }
            Which::Or(Ok(pair)) => {
                compile_expr(
                    pair.reborrow().get_first()?,
                    sh_args,
                    functions,
                    output_frame,
                )
                .await?;
                let jump_idx = output_frame.len();
                output_frame.push(Inst::JumpIfOk(0)); // patched below
                compile_expr(
                    pair.reborrow().get_second()?,
                    sh_args,
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
