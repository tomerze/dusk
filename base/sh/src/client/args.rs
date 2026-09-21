use crate::client::Created;
use crate::entry::ShEntriesBuilder;
use crate::{ArgsDataBuilder, PROGRAM_ID, sh_capnp};
use dusk_capnp::capnp_rpc;
use dusk_capnp::dusk_capnp::{created, dusk};
use dusk_capnp::pry;
use dusk_program::IntoCapnp;
use dusk_program::anyhow::{self, Context};
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh_bytecode::bytecode;
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
    bytecode::read(bytecode).context("a script's bytecode is not a Script message")
}

#[derive(dusk_program_proc::Args)]
pub struct ShArgs<S: ShEntriesBuilder> {
    #[data]
    pub data: ArgsDataBuilder,
    #[created]
    pub created: Option<created::Client>,
    pub client: dusk::Client,
    pub sh_entries_builder: S,
}

impl<S: ShEntriesBuilder> ShArgs<S> {
    pub fn new(client: dusk::Client, sh_entries_builder: S, mode: ShMode) -> anyhow::Result<Self> {
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
                        sh_entries_builder: sh_entries_builder.clone(),
                        server_pid,
                    }));
                }
            }
        }
        Ok(Self {
            data,
            created,
            client,
            sh_entries_builder,
        })
    }
}

pub async fn program_args_for_command<S: ShEntriesBuilder>(
    client: dusk::Client,
    sh_entries_builder: S,
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
    let builder = sh_entries_builder
        .get_entries()
        .into_iter()
        .find(|entry| entry.info.name == *program)
        .map(|entry| entry.program_args_builder)
        .ok_or_else(|| anyhow::anyhow!("no sh entry found for `{program}`"))?;
    let arg_refs: Vec<&str> = words[1..].to_vec();
    builder
        .build(client, &arg_refs)
        .await
        .context("program args builder failed")
}

#[dusk_program_proc::impl_args_rpc_server]
impl<S: ShEntriesBuilder> ShArgs<S> {
    fn compiler(
        &mut self,
        _params: sh_capnp::sh_args::server::CompilerParams,
        mut results: sh_capnp::sh_args::server::CompilerResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        results.get().set_result(capnp_rpc::new_client(ShCompiler {
            client: self.client.clone(),
            sh_entries_builder: self.sh_entries_builder.clone(),
        }));
        capnp::capability::Promise::ok(())
    }
}

pub struct ShCompiler<S: ShEntriesBuilder> {
    pub client: dusk::Client,
    pub sh_entries_builder: S,
}

impl<S: ShEntriesBuilder + 'static> sh_capnp::compiler::Server for ShCompiler<S> {
    fn build_program_args(
        &mut self,
        params: sh_capnp::compiler::BuildProgramArgsParams,
        mut results: sh_capnp::compiler::BuildProgramArgsResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        let command = pry!(pry!(pry!(params.get()).get_command()).to_str()).to_string();
        let client = self.client.clone();
        let sh_entries_builder = self.sh_entries_builder.clone();
        capnp::capability::Promise::from_future(async move {
            let program_args = program_args_for_command(client, sh_entries_builder, &command)
                .await
                .into_capnp()?;
            program_args.with_reader(|reader| results.get().set_program_args(reader))?;
            Ok(())
        })
    }
}
