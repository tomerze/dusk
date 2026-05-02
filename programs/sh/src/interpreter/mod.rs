use alloc::rc::Rc;
use alloc::string::String;
use core::cell::RefCell;
use core::future::Future;
use core::pin::Pin;
use dusk_program::anyhow::{Result, anyhow};
use hashbrown::HashMap;

use crate::sh_capnp;
use crate::sh_capnp::script;
use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::dusk_capnp::stream;

use execution::ExecutionError;
use inst::{Frame, Inst};

mod compiler;
mod execution;
mod inst;

#[derive(Clone)]
pub struct Interpreter {
    client: dusk::Client,
    sh_args: sh_capnp::sh_args::Client,
    functions: Rc<RefCell<HashMap<String, Rc<Frame>>>>,
}

impl Interpreter {
    pub fn new(client: dusk::Client, sh_args: sh_capnp::sh_args::Client) -> Self {
        Interpreter {
            client,
            sh_args,
            functions: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    pub async fn exec(
        &self,
        script: script::Reader<'_>,
        output: stream::Client,
        detached: bool,
    ) -> Result<()> {
        let frame = compiler::compile(script, self.sh_args.clone()).await?;
        tracing::debug!(
            dump = %inst::format_instructions(&frame),
            "frame disassembly"
        );
        let frame = Rc::new(frame);
        let _ = self.exec_inner(frame, output.clone(), detached).await;
        output.done_request().send().promise.await?;
        Ok(())
    }

    fn exec_inner<'a>(
        &'a self,
        frame: Rc<Frame>,
        output: stream::Client,
        detached: bool,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(async move {
            let mut current_frame = frame;
            let mut pc = 0usize; // Program counter, at the current frame
            let mut result_register: Result<()> = Ok(());

            loop {
                if pc >= current_frame.len() {
                    return result_register;
                }
                match &current_frame[pc] {
                    Inst::ProgramArgs(program_args) => {
                        let exec = execution::Execution::new(
                            self.client.clone(),
                            output.clone(),
                            detached,
                        );
                        result_register = match exec.program_args(program_args.clone()).await {
                            Ok(()) => Ok(()),
                            Err(ExecutionError::Runtime(e)) => {
                                tracing::error!(pc, error = %e, "runtime error during program execution");
                                Err(e)
                            }
                            Err(ExecutionError::Program(e)) => Err(e),
                        };
                        pc += 1;
                    }
                    Inst::Call(symbol) => {
                        let frame = self.functions.borrow().get(symbol).cloned();
                        match frame {
                            Some(frame) => {
                                result_register =
                                    self.exec_inner(frame, output.clone(), detached).await;
                            }
                            None => {
                                result_register = Err(anyhow!("unknown function: {}", symbol));
                            }
                        }
                        pc += 1;
                    }
                    Inst::TailCall(symbol) => {
                        let frame = self.functions.borrow().get(symbol).cloned();
                        match frame {
                            Some(frame) => {
                                current_frame = frame;
                                pc = 0;
                            }
                            None => {
                                result_register = Err(anyhow!("unknown function: {}", symbol));
                                pc += 1;
                            }
                        }
                    }
                    Inst::DefineFunction { symbol, frame } => {
                        self.functions
                            .borrow_mut()
                            .insert(symbol.clone(), frame.clone());
                        result_register = Ok(());
                        pc += 1;
                    }
                    Inst::JumpIfError(target) => {
                        pc = if result_register.is_err() {
                            *target
                        } else {
                            pc + 1
                        };
                    }
                    Inst::JumpIfOk(target) => {
                        pc = if result_register.is_ok() {
                            *target
                        } else {
                            pc + 1
                        };
                    }
                }
            }
        })
    }
}
