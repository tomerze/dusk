extern crate linkme;

use super::*;
use crate::entry::{ProgramArgsBuilder, ProgramInfo, ShEntry};
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use linkme::distributed_slice;
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
    fn build(
        &self,
        client: dusk::Client,
        args: &[&str],
    ) -> anyhow::Result<dusk_capnp::dusk_capnp::program_args::Client> {
        let _cli = ShCli::try_parse_from(args)?;
        let client: sh_capnp::sh_args::Client = capnp_rpc::new_client(ShArgs { client });
        Ok(client.cast_to::<dusk_capnp::dusk_capnp::program_args::Client>())
    }
}

#[distributed_slice(crate::entry::SH_ENTRIES)]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: ProgramInfo {
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
