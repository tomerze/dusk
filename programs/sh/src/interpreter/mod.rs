use dusk_program::anyhow::Result;

use crate::sh_capnp::script;
use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::dusk_capnp::stream;

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
                    result_register = exec.program_args(program_args.clone()).await;
                    if let Some(e) = result_register.as_ref().err() {
                        tracing::warn!(
                            pc = pc,
                            error = e.to_string(),
                            "program execution exited with error"
                        )
                    }
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
