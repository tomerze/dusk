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

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(sh_capnp::PROGRAM_ID),
            name: "sh",
            short_description: "run Dusk shell commands",
            long_description: r#"
`sh` runs commands in the Dusk shell — Dusk's own shell language, not a Unix
shell. Dusk shell commands run Dusk programs built into the Dusk Node.

* Use `sh <command>` (or `sh "<command>"`) to run a command.
* Use `sh -d <command>` to run it detached from the current session.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(ShProgramArgsBuilder {}),
    }
}
