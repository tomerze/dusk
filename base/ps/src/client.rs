use super::*;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
use std::rc::Rc;

fn parse_hex_or_decimal(s: &str) -> Result<u64, String> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).map_err(|e| e.to_string())
    } else {
        s.parse::<u64>().map_err(|e| e.to_string())
    }
}

#[derive(clap::Parser)]
#[command(name = "ps", no_binary_name = true)]
struct PsCli {
    /// Show only the process with this PID; omit to list all processes.
    #[arg(value_parser = parse_hex_or_decimal)]
    pid: Option<u64>,
}

struct PsProgramArgsBuilder {}

impl ProgramArgsBuilder for PsProgramArgsBuilder {
    fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = PsCli::try_parse_from(args)?;
        Ok(Args::new(cli.pid).as_program_args()?)
    }
}

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(ps_capnp::PROGRAM_ID),
            name: "ps",
            short_description: "list processes",
            long_description: r#"
The `ps` command is used to display information about the currently running processes.
* Use `ps` to list all currently running processes.
* Use `ps <pid>` to show only the process with that PID.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(PsProgramArgsBuilder {}),
    }
}
