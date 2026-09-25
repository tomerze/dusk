use crate::client::Created;
use crate::entry::sh_entries;
use crate::{ArgsDataBuilder, PROGRAM_ID};
use dusk_capnp::capnp_rpc;
use dusk_capnp::dusk_capnp::{created, dusk};
use dusk_program::anyhow::{self, Context};
use dusk_program::program_args::ProgramArgs;
use std::rc::Rc;
use std::string::String;
use std::vec::Vec;

pub enum ShMode {
    Server,
    Script(Vec<u8>),
    DetachedScript(Vec<u8>),
    Prompt {
        client_hostname: String,
        server_pid: u64,
    },
}

fn read_bytecode(
    bytecode: &[u8],
) -> anyhow::Result<capnp::message::Reader<dusk_capnp::capnp::serialize::OwnedSegments>> {
    dusk_program_sh_compiler::read(bytecode).context("a script's bytecode is not a Script message")
}

#[derive(dusk_program_proc::Args)]
pub struct ShArgs {
    #[data]
    pub data: ArgsDataBuilder,
    #[created]
    pub created: Option<created::Client>,
    pub client: dusk::Client,
}

impl ShArgs {
    pub fn new(client: dusk::Client, mode: ShMode) -> anyhow::Result<Self> {
        let mut data = ArgsDataBuilder::new_default();
        let mut created = None;
        {
            let mut data_builder = data.init_root();
            match mode {
                ShMode::Server => data_builder.set_server(()),
                ShMode::Script(bytecode) => {
                    let message = read_bytecode(&bytecode)?;
                    data_builder.set_script(message.get_root()?)?;
                }
                ShMode::DetachedScript(bytecode) => {
                    let message = read_bytecode(&bytecode)?;
                    data_builder.set_detached_script(message.get_root()?)?;
                }
                ShMode::Prompt {
                    client_hostname,
                    server_pid,
                } => {
                    data_builder.set_prompt(&client_hostname);
                    created = Some(capnp_rpc::new_client(Created {
                        client: client.clone(),
                        server_pid,
                    }));
                }
            }
        }
        Ok(Self {
            data,
            created,
            client,
        })
    }
}

pub async fn program_args_for_command(
    client: dusk::Client,
    command: &str,
) -> anyhow::Result<Rc<ProgramArgs>> {
    let (remaining, words) = crate::parser::command_words(command)
        .map_err(|_| anyhow::anyhow!("invalid command `{}`", command))?;
    if !remaining.trim().is_empty() {
        anyhow::bail!("invalid command `{command}`");
    }
    let program = words
        .first()
        .ok_or_else(|| anyhow::anyhow!("empty command"))?;
    let builder = sh_entries()
        .iter()
        .find(|entry| entry.info.name == *program)
        .map(|entry| entry.program_args_builder.clone())
        .ok_or_else(|| anyhow::anyhow!("no sh entry found for `{program}`"))?;
    let arg_refs: Vec<&str> = words[1..].to_vec();
    builder
        .build(client, &arg_refs)
        .await
        .context("program args builder failed")
}
