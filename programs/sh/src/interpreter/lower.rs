use alloc::rc::Rc;
use alloc::string::{String, ToString};
use core::future::Future;
use core::pin::Pin;
use dusk_program::anyhow::{Result, anyhow};
use hashbrown::HashSet;

use crate::sh_capnp;
use crate::sh_capnp::script;

use super::inst::{Frame, Inst};

pub async fn lower(
    script: script::Reader<'_>,
    sh_args: sh_capnp::sh_args::Client,
) -> Result<Frame> {
    let mut functions = HashSet::new();
    let mut frame = Frame::new();
    lower_script(script, &sh_args, &mut functions, &mut frame).await?;
    optimize_tail_call(&mut frame);
    Ok(frame)
}

fn optimize_tail_call(frame: &mut Frame) {
    if let Some(last) = frame.last_mut()
        && let Inst::Call(name) = last
    {
        let name = core::mem::take(name);
        *last = Inst::TailCall(name);
    }
}

fn lower_script<'a>(
    script: script::Reader<'a>,
    sh_args: &'a sh_capnp::sh_args::Client,
    functions: &'a mut HashSet<String>,
    frame: &'a mut Frame,
) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
    Box::pin(async move {
        for statement in script.get_statements()?.iter() {
            lower_statement(statement, sh_args, functions, frame).await?;
        }
        Ok(())
    })
}

async fn lower_statement<'a>(
    statement: script::statement::Reader<'a>,
    sh_args: &sh_capnp::sh_args::Client,
    functions: &mut HashSet<String>,
    frame: &mut Frame,
) -> Result<()> {
    use script::statement::Which;
    match statement.which()? {
        Which::Expr(expr) => lower_expr(expr?, sh_args, functions, frame).await,
        Which::FunctionDefinition(def) => {
            let def = def?;
            let name = def.get_name()?.to_str()?.to_string();
            functions.insert(name.clone());
            let mut function_frame = Frame::new();
            lower_script(def.get_body()?, sh_args, functions, &mut function_frame).await?;
            optimize_tail_call(&mut function_frame);
            frame.push(Inst::DefineFunction {
                name,
                frame: Rc::new(function_frame),
            });
            Ok(())
        }
    }
}

fn lower_expr<'a>(
    expr: script::statement::expr::Reader<'a>,
    sh_args: &'a sh_capnp::sh_args::Client,
    functions: &'a mut HashSet<String>,
    frame: &'a mut Frame,
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
                    frame.push(Inst::Call(first_word.to_string()));
                } else {
                    let mut request = sh_args.build_program_args_request();
                    request.get().set_command(text);
                    let reply = request.send().promise.await?;
                    let program_args = reply.get()?.get_program_args()?;
                    frame.push(Inst::ProgramArgs(program_args));
                }
                Ok(())
            }
            Which::And(Ok(pair)) => {
                lower_expr(pair.reborrow().get_first()?, sh_args, functions, frame).await?;
                let jump_idx = frame.len();
                frame.push(Inst::JumpIfError(0)); // patched below
                lower_expr(pair.reborrow().get_second()?, sh_args, functions, frame).await?;
                let target = frame.len();
                frame[jump_idx] = Inst::JumpIfError(target);
                Ok(())
            }
            Which::Or(Ok(pair)) => {
                lower_expr(pair.reborrow().get_first()?, sh_args, functions, frame).await?;
                let jump_idx = frame.len();
                frame.push(Inst::JumpIfOk(0)); // patched below
                lower_expr(pair.reborrow().get_second()?, sh_args, functions, frame).await?;
                let target = frame.len();
                frame[jump_idx] = Inst::JumpIfOk(target);
                Ok(())
            }
            _ => Err(anyhow!("error lowering expression")),
        }
    })
}
