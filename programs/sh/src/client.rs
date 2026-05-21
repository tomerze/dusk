
use super::*;
use crate::ShMode;
use crate::entry::{EntryInfo, ProgramArgsBuilder, ShEntry, StaticShEntriesBuilder};
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use std::rc::Rc;

#[derive(clap::Parser)]
#[command(name = "sh", no_binary_name = true)]
struct ShCli {
    #[arg(short = 'd', long = "detach")]
    detach: bool,
    command: Option<String>,
}

struct ShProgramArgsBuilder {}

impl ProgramArgsBuilder for ShProgramArgsBuilder {
    fn build(&self, client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = ShCli::try_parse_from(args)?;
        let mode = match cli.command {
            None => ShMode::Server,
            Some(command) if cli.detach => ShMode::DetachedScript(command),
            Some(command) => ShMode::Script(command),
        };
        Ok(ShArgs::new(client, StaticShEntriesBuilder::default(), mode)?.as_program_args()?)
    }
}

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(sh_capnp::PROGRAM_ID),
            name: "sh",
            short_description: "run a shell command",
            long_description: r#"
The `sh` command is used to run shell commands.
* Use `sh <command>` (or `sh "<command>"`) to run a command
* Use `sh -d <command>` to run a command detached from the current session"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(ShProgramArgsBuilder {}),
    }
}
