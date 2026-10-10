use crate::client::{Created, client_hostname};
use crate::entry::ProgramArgsBuilder;
use crate::sh_capnp::DEFAULT_PID;
use crate::{ShArgs, ShMode};
use clap::Parser as _;
use dusk_capnp::dusk_capnp::dusk;
use dusk_program::anyhow;
use dusk_program::program_args::ProgramArgs;
use std::format;
use std::rc::Rc;
use std::string::String;

#[derive(clap::Parser)]
#[command(name = "sh", no_binary_name = true)]
struct ShCli {
    #[arg(short = 'd', long = "detach")]
    detach: bool,
    #[arg(long = "server", value_name = "PID", num_args = 0..=1, value_parser = parse_pid, conflicts_with_all = ["prompt", "detach"])]
    server: Option<Option<u64>>,
    #[arg(long = "prompt", value_name = "PID", num_args = 0..=1, value_parser = parse_pid, conflicts_with = "detach")]
    prompt: Option<Option<u64>>,
    command: Option<String>,
}

pub fn parse_pid(pid: &str) -> Result<u64, String> {
    let parsed = match pid.strip_prefix("0x").or_else(|| pid.strip_prefix("0X")) {
        Some(digits) => u64::from_str_radix(digits, 16),
        None => pid.parse(),
    };
    parsed.map_err(|error| format!("`{pid}` is not a pid: {error}"))
}

pub struct ShProgramArgsBuilder {}

#[dusk_program::async_trait::async_trait(?Send)]
impl ProgramArgsBuilder for ShProgramArgsBuilder {
    async fn build(&self, client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = ShCli::try_parse_from(args)?;
        if let Some(server_pid) = cli.server {
            let program_args = ShArgs::new(ShMode::Server)?.as_program_args()?;
            program_args.set_pid(Some(server_pid.unwrap_or(DEFAULT_PID)))?;
            return Ok(program_args);
        }
        if let Some(server_pid) = cli.prompt {
            return Ok(ShArgs::new(ShMode::Prompt {
                client_hostname: client_hostname(),
                created: dusk_capnp::capnp_rpc::new_client(Created {
                    client: client.clone(),
                    server_pid: server_pid.unwrap_or(DEFAULT_PID),
                }),
            })?
            .as_program_args()?);
        }
        let mode = match cli.command {
            None => anyhow::bail!("sh takes a command, --server or --prompt"),
            Some(command) => {
                let script = crate::client::args::compile(client.clone(), &command).await?;
                if cli.detach {
                    ShMode::DetachedScript(script)
                } else {
                    ShMode::Script(script)
                }
            }
        };
        Ok(ShArgs::new(mode)?.as_program_args()?)
    }
}
