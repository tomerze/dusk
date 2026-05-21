
use super::*;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
use std::rc::Rc;

#[derive(clap::Parser)]
#[command(name = "sleep", no_binary_name = true)]
struct SleepCli {
    /// Milliseconds to sleep
    duration_ms: u64,
}

struct SleepProgramArgsBuilder {}

impl ProgramArgsBuilder for SleepProgramArgsBuilder {
    fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = SleepCli::try_parse_from(args)?;
        Ok(Args::duration_ms(cli.duration_ms).as_program_args()?)
    }
}

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(sleep_capnp::PROGRAM_ID),
            name: "sleep",
            short_description: "sleep for a duration",
            long_description: r#"
The `sleep` command pauses for `<ms>` milliseconds. Then exists.

Example:
* `sleep 1000`   sleep one second
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(SleepProgramArgsBuilder {}),
    }
}
