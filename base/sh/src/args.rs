use alloc::string::String;
use alloc::vec::Vec;

use dusk_capnp::dusk_capnp::created;
use dusk_program::anyhow::{self, Context};

use crate::{ArgsDataBuilder, PROGRAM_ID, sh_capnp};

pub enum ShMode {
    Server,
    Script(Vec<u8>),
    DetachedScript(Vec<u8>),
    Prompt {
        client_hostname: String,
        created: created::Client,
    },
}

#[derive(dusk_program_proc::Args)]
pub struct ShArgs {
    #[data]
    pub data: ArgsDataBuilder,
    #[created]
    pub created: Option<created::Client>,
}

impl ShArgs {
    pub fn new(mode: ShMode) -> anyhow::Result<Self> {
        let mut data = ArgsDataBuilder::new_default();
        let created = {
            let mut data_builder = data.init_root();
            match mode {
                ShMode::Server => {
                    data_builder.set_server(());
                    None
                }
                ShMode::Script(script) => {
                    let message = dusk_program_sh_compiler::read(&script)
                        .context("a script is not compiled bytecode")?;
                    data_builder.set_script(message.get_root()?)?;
                    None
                }
                ShMode::DetachedScript(script) => {
                    let message = dusk_program_sh_compiler::read(&script)
                        .context("a script is not compiled bytecode")?;
                    data_builder.set_detached_script(message.get_root()?)?;
                    None
                }
                ShMode::Prompt {
                    client_hostname,
                    created,
                } => {
                    data_builder.set_prompt(&client_hostname);
                    Some(created)
                }
            }
        };
        Ok(Self { data, created })
    }
}

#[dusk_program_proc::impl_args_rpc_server]
impl ShArgs {}
