use alloc::rc::Rc;
use alloc::string::String;
use alloc::sync::Arc;
use core::cell::RefCell;
use core::future::Future;
use core::pin::Pin;
use dusk_program::anyhow::Result;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::embassy_sync::mutex::Mutex;
use hashbrown::{HashMap, HashSet};

use crate::sh_capnp;
use crate::sh_capnp::script;
use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::dusk_capnp::stream;

use execution::ExecutionError;
pub use execution::Stop;
use inst::{Frame, Inst, ScriptWrapper};

mod compiler;
mod execution;
mod inst;

pub(crate) type FunctionTable =
    Arc<Mutex<CriticalSectionRawMutex, HashMap<String, Arc<ScriptWrapper>>>>;

pub(crate) type CompiledFunctions = Rc<RefCell<HashMap<String, Arc<Frame>>>>;

#[derive(Clone)]
pub struct Interpreter {
    client: dusk::Client,
    function_table: FunctionTable,
    compiled_functions: CompiledFunctions,
}

impl Interpreter {
    pub(crate) fn new(client: dusk::Client, function_table: FunctionTable) -> Self {
        Interpreter {
            client,
            function_table,
            compiled_functions: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    async fn resolve_function(
        &self,
        symbol: &str,
        compiler: &sh_capnp::compiler::Client,
    ) -> Result<Arc<Frame>> {
        if let Some(frame) = self.compiled_functions.borrow().get(symbol).cloned() {
            return Ok(frame);
        }
        compiler::compile_function(
            &self.function_table,
            &self.compiled_functions,
            compiler.clone(),
            symbol,
        )
        .await
    }

    #[allow(clippy::arc_with_non_send_sync)]
    pub async fn exec(
        &self,
        script: script::Reader<'_>,
        output: stream::Client,
        stop: &Stop,
        compiler: sh_capnp::compiler::Client,
    ) -> Result<()> {
        let symbols: HashSet<String> = self.function_table.lock().await.keys().cloned().collect();
        let frame = compiler::compile(script, compiler.clone(), symbols).await?;
        tracing::debug!(
            dump = %inst::format_instructions(&frame),
            "script frame disassembly"
        );
        let frame = Arc::new(frame);
        self.exec_inner(frame, output, stop, compiler).await
    }

    fn exec_inner<'a>(
        &'a self,
        frame: Arc<Frame>,
        output: stream::Client,
        stop: &'a Stop,
        compiler: sh_capnp::compiler::Client,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(async move {
            let mut current_frame = frame;
            let mut pc = 0usize;
            let mut result_register: Result<()> = Ok(());

            loop {
                dusk_program::embassy_futures::yield_now().await;
                if stop.signaled() {
                    return Ok(());
                }
                if pc >= current_frame.len() {
                    return result_register;
                }
                match &current_frame[pc] {
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
                        match self.resolve_function(&symbol, &compiler).await {
                            Ok(frame) => {
                                result_register = self
                                    .exec_inner(frame, output.clone(), stop, compiler.clone())
                                    .await;
                            }
                            Err(e) => result_register = Err(e),
                        }
                        pc += 1;
                    }
                    Inst::TailCall(symbol) => {
                        let symbol = symbol.clone();
                        match self.resolve_function(&symbol, &compiler).await {
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
                        let symbol = symbol.clone();
                        let body = body.clone();
                        let is_empty = body
                            .0
                            .get_root_as_reader::<script::Reader<'_>>()?
                            .get_statements()?
                            .is_empty();
                        if is_empty {
                            self.function_table.lock().await.remove(&symbol);
                            self.compiled_functions.borrow_mut().remove(&symbol);
                        } else {
                            let previous = self
                                .function_table
                                .lock()
                                .await
                                .insert(symbol.clone(), body);
                            if previous.is_some() {
                                tracing::info!(symbol = %symbol, "overwriting existing function");
                            }
                            // Drop any stale compiled frame so compile_function
                            // recompiles against the new body.
                            self.compiled_functions.borrow_mut().remove(&symbol);
                            if let Err(e) = compiler::compile_function(
                                &self.function_table,
                                &self.compiled_functions,
                                compiler.clone(),
                                &symbol,
                            )
                            .await
                            {
                                tracing::error!(error = %e, "function compilation failed");
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
