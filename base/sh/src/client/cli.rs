use crate::entry::{ProgramArgsBuilder, StaticShEntriesBuilder};
use crate::{ShArgs, ShMode};
use clap::Parser as _;
use dusk_capnp::dusk_capnp::dusk;
use dusk_program::anyhow;
use dusk_program::program_args::ProgramArgs;
use std::rc::Rc;
use std::string::String;

#[derive(clap::Parser)]
#[command(name = "sh", no_binary_name = true)]
struct ShCli {
    #[arg(short = 'd', long = "detach")]
    detach: bool,
    command: Option<String>,
}

pub struct ShProgramArgsBuilder {}

#[dusk_program::async_trait::async_trait(?Send)]
impl ProgramArgsBuilder for ShProgramArgsBuilder {
    async fn build(&self, client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = ShCli::try_parse_from(args)?;
        let mode = match cli.command {
            None => ShMode::Server,
            Some(command) if cli.detach => ShMode::DetachedScript(command),
            Some(command) => ShMode::Script(command),
        };
        Ok(ShArgs::new(client, StaticShEntriesBuilder::default(), mode)?.as_program_args()?)
    }
}
