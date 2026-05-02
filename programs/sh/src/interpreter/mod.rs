use alloc::sync::Arc;
use alloc::string::String;
use core::future::Future;
use core::pin::Pin;
use dusk_program::anyhow::{Result, anyhow};
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::embassy_sync::mutex::Mutex;
use hashbrown::{HashMap, HashSet};

use crate::sh_capnp;
use crate::sh_capnp::script;
use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::dusk_capnp::stream;

use execution::ExecutionError;
use inst::{Frame, Inst, ScriptMessage};

mod compiler;
mod execution;
mod inst;

pub type FunctionTable =
    Arc<Mutex<CriticalSectionRawMutex, HashMap<String, Arc<ScriptMessage>>>>;

#[derive(Clone)]
pub struct Interpreter {
    client: dusk::Client,
    sh_args: sh_capnp::sh_args::Client,
    function_table: FunctionTable,
}

impl Interpreter {
    pub fn new(
        client: dusk::Client,
        sh_args: sh_capnp::sh_args::Client,
        function_table: FunctionTable,
    ) -> Self {
        Interpreter {
            client,
            sh_args,
            function_table,
        }
    }

    pub async fn exec(
        &self,
        script: script::Reader<'_>,
        output: stream::Client,
        detached: bool,
    ) -> Result<()> {
        let symbols: HashSet<String> =
            self.function_table.lock().await.keys().cloned().collect();
        let frame = compiler::compile(script, self.sh_args.clone(), symbols).await?;
        tracing::debug!(
            dump = %inst::format_instructions(&frame),
            "frame disassembly"
        );
        let frame = Arc::new(frame);
        let _ = self.exec_inner(frame, output.clone(), detached).await;
        output.done_request().send().promise.await?;
        Ok(())
    }

    async fn compile_function(&self, symbol: &str) -> Result<Arc<Frame>> {
        let body = self
            .function_table
            .lock()
            .await
            .get(symbol)
            .cloned()
            .ok_or_else(|| anyhow!("unknown function: {}", symbol))?;
        let symbols: HashSet<String> =
            self.function_table.lock().await.keys().cloned().collect();
        let script = body.0.get_root_as_reader::<script::Reader<'_>>()?;
        let frame = compiler::compile(script, self.sh_args.clone(), symbols).await?;
        Ok(Arc::new(frame))
    }

    fn exec_inner<'a>(
        &'a self,
        frame: Arc<Frame>,
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
                        let symbol = symbol.clone();
                        match self.compile_function(&symbol).await {
                            Ok(frame) => {
                                result_register =
                                    self.exec_inner(frame, output.clone(), detached).await;
                            }
                            Err(e) => result_register = Err(e),
                        }
                        pc += 1;
                    }
                    Inst::TailCall(symbol) => {
                        let symbol = symbol.clone();
                        match self.compile_function(&symbol).await {
                            Ok(frame) => {
                                current_frame = frame;
                                pc = 0;
                            }
                            Err(e) => {
                                result_register = Err(e);
                                pc += 1;
                            }
                        }
                    }
                    Inst::DefineFunction { symbol, body } => {
                        self.function_table
                            .lock()
                            .await
                            .insert(symbol.clone(), body.clone());
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
