use alloc::string::String;
use alloc::vec::Vec;

use dusk_capnp::capnp::message::HeapAllocator;
use dusk_capnp::capnp_rpc::ImbuedMessageBuilder;
use dusk_capnp::dusk_capnp::created;
use dusk_program::anyhow::{self, Context};

use crate::{PROGRAM_ID, sh_capnp};

pub enum ShMode {
    Server,
    Script(Vec<u8>),
    DetachedScript(Vec<u8>),
    Prompt {
        client_hostname: String,
        created: created::Client,
    },
}

pub type ShArgsDataMessage = ImbuedMessageBuilder<HeapAllocator>;

#[derive(dusk_program_proc::Args)]
pub struct ShArgs {
    #[data]
    pub data: ShArgsDataMessage,
    #[created]
    pub created: Option<created::Client>,
}

impl ShArgs {
    pub fn new(mode: ShMode) -> anyhow::Result<Self> {
        let mut data = ShArgsDataMessage::new(HeapAllocator::new());
        let created = {
            let mut data_builder: sh_capnp::sh_args::data::Builder = data.get_root()?;
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
