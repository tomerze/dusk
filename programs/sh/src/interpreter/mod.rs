use dusk_program::anyhow::Result;

use crate::sh_capnp::script;
use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::dusk_capnp::stream;

use execution::ExecutionError;
use inst::Inst;

mod execution;
mod inst;
mod lower;

#[derive(Clone)]
pub struct Interpreter {
    client: dusk::Client,
}

impl Interpreter {
    pub fn new(client: dusk::Client) -> Self {
        Interpreter { client }
    }

    pub async fn exec(&self, script: script::Reader<'_>, output: stream::Client) -> Result<()> {
        let instructions = lower::lower(script, &output)?;
        let mut pc = 0;
        let mut result_register: Result<()> = Ok(());

        while pc < instructions.len() {
            match &instructions[pc] {
                Inst::Command { program_args, mode } => {
                    let exec = execution::Execution::new(self.client.clone(), mode.clone());
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

        output.done_request().send().promise.await?;
        Ok(())
    }
}
