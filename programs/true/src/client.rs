extern crate linkme;

use super::*;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{ProgramArgsBuilder, ProgramInfo, ShEntry};
use linkme::distributed_slice;
use std::rc::Rc;

#[derive(clap::Parser)]
#[command(name = "true", no_binary_name = true)]
struct TrueCli {}

struct TrueProgramArgsBuilder {}

impl ProgramArgsBuilder for TrueProgramArgsBuilder {
    fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let _cli = TrueCli::try_parse_from(args)?;
        Ok(Args::new().as_program_args()?)
    }
}

#[distributed_slice(dusk_program_sh::entry::SH_ENTRIES)]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: ProgramInfo {
            program_id: Some(true_capnp::PROGRAM_ID),
            name: "true",
            short_description: "do nothing successfully",
            long_description: r#"
The `true` command does nothing and exits successfully. Useful as a placeholder
in shell scripts where a command is syntactically required but no behaviour is.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(TrueProgramArgsBuilder {}),
    }
}
