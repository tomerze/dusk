use alloc::rc::Rc;
use alloc::string::{String, ToString};
use core::future::Future;
use core::pin::Pin;
use dusk_program::anyhow::{Result, anyhow};
use hashbrown::HashSet;

use crate::sh_capnp;
use crate::sh_capnp::script;

use super::inst::{Frame, Inst};

pub async fn compile(
    script: script::Reader<'_>,
    sh_args: sh_capnp::sh_args::Client,
) -> Result<Frame> {
    let mut functions = HashSet::new();
    let mut output_frame = Frame::new();
    compile_script(script, &sh_args, &mut functions, &mut output_frame).await?;
    optimize_tail_call(&mut output_frame);
    Ok(output_frame)
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
            let mut function_frame = Frame::new();
            compile_script(def.get_body()?, sh_args, functions, &mut function_frame).await?;
            optimize_tail_call(&mut function_frame);
            output_frame.push(Inst::DefineFunction {
                symbol,
                frame: Rc::new(function_frame),
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
