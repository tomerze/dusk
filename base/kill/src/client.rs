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
#[command(name = "kill", no_binary_name = true)]
struct KillCli {
    /// PID of the process to signal
    #[arg(value_parser = parse_hex_or_decimal)]
    pid: u64,
    /// Signal number to send (default: 15, SIGTERM)
    #[arg(long, default_value_t = 15, value_parser = parse_hex_or_decimal)]
    signal: u64,
}

struct KillProgramArgsBuilder {}

#[dusk_program::async_trait::async_trait(?Send)]
impl ProgramArgsBuilder for KillProgramArgsBuilder {
    async fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = KillCli::try_parse_from(args)?;
        Ok(Args::new(cli.pid, cli.signal).as_program_args()?)
    }
}

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(kill_capnp::PROGRAM_ID),
            name: "kill",
            short_description: "send a signal to a process",
            long_description: r#"
The `kill` program is used to send a signal to a process.
By default, it sends the `Terminate` (15) signal, which requests that the process terminates.
You can specify a different signal using the `--signal <signal>` option.
* Use `kill <pid>` to send the `Terminate` (15) signal to the process with the specified `<pid>`.
* Use `kill --signal <signal> <pid>` to send the signal corresponding to `<signal>` to the process.
* Use `kill --signal 8 <pid>` to send the `Reap` (8) signal, which reaps a zombie process logging it's exit result. Does nothing to non-zombie processes
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(KillProgramArgsBuilder {}),
    }
}
