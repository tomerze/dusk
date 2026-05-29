use super::*;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
use std::rc::Rc;

#[derive(clap::Parser)]
#[command(name = "hostname", no_binary_name = true)]
struct HostnameCli {}

struct HostnameProgramArgsBuilder {}

impl ProgramArgsBuilder for HostnameProgramArgsBuilder {
    fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let _cli = HostnameCli::try_parse_from(args)?;
        Ok(Args::new().as_program_args()?)
    }
}

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(hostname_capnp::PROGRAM_ID),
            name: "hostname",
            short_description: "show the hostname",
            long_description: r#"
The `hostname` command prints the hostname of the dusk node.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(HostnameProgramArgsBuilder {}),
    }
}
