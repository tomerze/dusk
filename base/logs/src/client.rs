use super::*;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
use std::rc::Rc;

#[derive(clap::Parser)]
#[command(name = "logs", no_binary_name = true)]
struct LogsCli {}

struct LogsProgramArgsBuilder {}

impl ProgramArgsBuilder for LogsProgramArgsBuilder {
    fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let _cli = LogsCli::try_parse_from(args)?;
        Ok(Args::new().as_program_args()?)
    }
}

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(logs_capnp::PROGRAM_ID),
            name: "logs",
            short_description: "do nothing",
            long_description: r#"
The `logs` command does nothing and exits successfully. It is an empty program
skeleton.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(LogsProgramArgsBuilder {}),
    }
}
