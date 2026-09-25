use alloc::rc::Rc;
use alloc::string::String;
use alloc::sync::Arc;
use core::cell::RefCell;
use core::future::Future;
use core::pin::Pin;
use dusk_program::anyhow::Result;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::embassy_sync::mutex::Mutex;
use hashbrown::HashMap;

use crate::bytecode;
use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::dusk_capnp::stream;

use execution::ExecutionError;
pub use execution::Stop;
use instructions::{CompiledScript, Inst, Instructions};

mod codegen;
mod execution;
mod instructions;

pub(crate) type FunctionTable =
    Arc<Mutex<CriticalSectionRawMutex, HashMap<String, Arc<CompiledScript>>>>;

pub(crate) type GeneratedFunctions = Rc<RefCell<HashMap<String, Arc<Instructions>>>>;

#[derive(Clone)]
pub struct Interpreter {
    client: dusk::Client,
    function_table: FunctionTable,
    generated_functions: GeneratedFunctions,
}

impl Interpreter {
    pub(crate) fn new(client: dusk::Client, function_table: FunctionTable) -> Self {
        Interpreter {
            client,
            function_table,
            generated_functions: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    async fn resolve_function(&self, symbol: &str) -> Result<Arc<Instructions>> {
        if let Some(instructions) = self.generated_functions.borrow().get(symbol).cloned() {
            return Ok(instructions);
        }
        codegen::generate_function(&self.function_table, &self.generated_functions, symbol).await
    }

    #[allow(clippy::arc_with_non_send_sync)]
    pub async fn exec(
        &self,
        script: bytecode::Reader<'_>,
        output: stream::Client,
        stop: &Stop,
    ) -> Result<()> {
        let instructions = codegen::generate(script)?;
        tracing::debug!(
            dump = %instructions::format_instructions(&instructions),
            "script instruction disassembly"
        );
        self.exec_inner(Arc::new(instructions), output, stop).await
    }

    fn exec_inner<'a>(
        &'a self,
        instructions: Arc<Instructions>,
        output: stream::Client,
        stop: &'a Stop,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(async move {
            let mut current_instructions = instructions;
            let mut pc = 0usize;
            let mut result_register: Result<()> = Ok(());

            loop {
                dusk_program::embassy_futures::yield_now().await;
                if stop.signaled() {
                    return Ok(());
                }
                if pc >= current_instructions.len() {
                    return result_register;
                }
                match &current_instructions[pc] {
                    Inst::ProgramArgs(program_args) => {
                        let exec = execution::Execution::new(self.client.clone(), output.clone());
                        result_register = match exec.program_args(program_args.clone(), stop).await
                        {
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
                        match self.resolve_function(&symbol).await {
                            Ok(instructions) => {
                                result_register =
                                    self.exec_inner(instructions, output.clone(), stop).await;
                            }
                            Err(e) => result_register = Err(e),
                        }
                        pc += 1;
                    }
                    Inst::TailCall(symbol) => {
                        let symbol = symbol.clone();
                        match self.resolve_function(&symbol).await {
                            Ok(instructions) => {
                                current_instructions = instructions;
                                pc = 0;
                            }
                            Err(e) => {
                                result_register = Err(e);
                                pc += 1;
                            }
                        }
                    }
                    Inst::DefineFunction { symbol, body } => {
                        let symbol = symbol.clone();
                        let body = body.clone();
                        let is_empty = body.root()?.get_statements()?.is_empty();
                        if is_empty {
                            self.function_table.lock().await.remove(&symbol);
                            self.generated_functions.borrow_mut().remove(&symbol);
                        } else {
                            let previous = self
                                .function_table
                                .lock()
                                .await
                                .insert(symbol.clone(), body);
                            if previous.is_some() {
                                tracing::info!(symbol = %symbol, "overwriting existing function");
                            }
                            // Drop any stale instructions so generate_function
                            // generates them again from the new body.
                            self.generated_functions.borrow_mut().remove(&symbol);
                            if let Err(e) = codegen::generate_function(
                                &self.function_table,
                                &self.generated_functions,
                                &symbol,
                            )
                            .await
                            {
                                tracing::error!(error = %e, "function generation failed");
                            }
                        }
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
