use super::*;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
use std::rc::Rc;

#[derive(clap::Parser)]
#[command(name = "sys", no_binary_name = true, disable_help_flag = true)]
struct SysCli {
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

struct SysProgramArgsBuilder;

#[dusk_program::async_trait::async_trait(?Send)]
impl ProgramArgsBuilder for SysProgramArgsBuilder {
    async fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = SysCli::try_parse_from(args)?;
        Ok(Args::new(&cli.command.join(" ")).as_program_args()?)
    }
}

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(sys_capnp::PROGRAM_ID),
            name: "sys",
            short_description: "run a command in the host shell",
            long_description: "Run `sys <command>` in the node's default shell without a PTY. Quote the command to pass shell operators and quoting through Dusk. Standard input is inherited from the node. Standard output and standard error are returned when the command exits. No host shell history is saved.",
            version: VERSION,
        },
        program_args_builder: Rc::new(SysProgramArgsBuilder),
    }
}
