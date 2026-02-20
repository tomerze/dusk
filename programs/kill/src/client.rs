extern crate linkme;

use super::*;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program_sh::entry::{ProgramArgsBuilder, ProgramInfo, ShEntry};
use linkme::distributed_slice;
use std::rc::Rc;

#[derive(clap::Parser)]
#[command(name = "kill", no_binary_name = true)]
struct KillCli {
    /// PID of the process to signal
    pid: u64,
    /// Signal number to send (default: 15, SIGTERM)
    #[arg(long, default_value_t = 15)]
    signal: u64,
}

struct KillProgramArgsBuilder {}

impl ProgramArgsBuilder for KillProgramArgsBuilder {
    fn build(
        &self,
        client: dusk::Client,
        args: &str,
    ) -> anyhow::Result<dusk_capnp::dusk_capnp::program_args::Client> {
        let cli = KillCli::try_parse_from(args.split_whitespace())?;
        let client: kill_capnp::kill_args::Client = capnp_rpc::new_client(Args {
            client,
            pid: cli.pid,
            signal: cli.signal,
        });
        Ok(client.cast_to::<dusk_capnp::dusk_capnp::program_args::Client>())
    }
}

#[distributed_slice(dusk_program_sh::entry::SH_ENTRIES)]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: ProgramInfo {
            program_id: Some(kill_capnp::PROGRAM_ID),
            name: "kill",
            short_description: "send a signal to a process",
            long_description: r#"
The `kill` command is used to send a signal to a process.
By default, it sends the `Terminate` (15) signal, which requests that the process terminates.
You can specify a different signal using the `-<signal>` option.
* Use `kill <pid>` to send the `Terminate` (15) signal to the process with the specified `<pid>`.
* Use `kill -<signal> <pid>` to send the signal corresponding to `<signal>` to the process.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(KillProgramArgsBuilder {}),
    }
}
