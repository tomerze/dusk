use super::*;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
use std::rc::Rc;

#[derive(clap::Parser)]
#[command(name = "echo", no_binary_name = true)]
struct EchoCli {
    /// Text to echo back; multiple words are joined with single spaces
    words: Vec<String>,
}

struct EchoProgramArgsBuilder {}

#[dusk_program::async_trait::async_trait(?Send)]
impl ProgramArgsBuilder for EchoProgramArgsBuilder {
    async fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = EchoCli::try_parse_from(args)?;
        Ok(Args::new(&cli.words.join(" ")).as_program_args()?)
    }
}

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(echo_capnp::PROGRAM_ID),
            name: "echo",
            short_description: "echo text back",
            long_description: r#"
The `echo` program prints its arguments back as a string.
* Use `echo <text>` to print `<text>`.
* Multiple words are joined with single spaces.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(EchoProgramArgsBuilder {}),
    }
}
