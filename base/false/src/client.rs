use super::*;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
use std::rc::Rc;

#[derive(clap::Parser)]
#[command(name = "false", no_binary_name = true)]
struct FalseCli {}

struct FalseProgramArgsBuilder {}

#[dusk_program::async_trait::async_trait(?Send)]
impl ProgramArgsBuilder for FalseProgramArgsBuilder {
    async fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let _cli = FalseCli::try_parse_from(args)?;
        Ok(Args::new().as_program_args()?)
    }
}

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(false_capnp::PROGRAM_ID),
            name: "false",
            short_description: "do nothing, unsuccessfully",
            long_description: r#"
The `false` command does nothing and exits with failure. Useful as a placeholder
in shell scripts where a command is syntactically required but must report
failure.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(FalseProgramArgsBuilder {}),
    }
}
