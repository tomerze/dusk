extern crate linkme;

use super::*;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{ProgramArgsBuilder, ProgramInfo, ShEntry};
use linkme::distributed_slice;
use std::rc::Rc;

const FORMAT: &str = "%Y-%m-%d %H:%M:%S";

#[derive(clap::Parser)]
#[command(name = "date", no_binary_name = true)]
struct DateCli {
    /// Set the clock to the given UTC timestamp in `YYYY-MM-DD HH:MM:SS` form.
    #[arg(short = 's', long = "set")]
    set: Option<String>,
}

struct DateProgramArgsBuilder {}

impl ProgramArgsBuilder for DateProgramArgsBuilder {
    fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = DateCli::try_parse_from(args)?;
        let args = match cli.set {
            None => Args::show(),
            Some(input) => {
                let naive = chrono::NaiveDateTime::parse_from_str(&input, FORMAT).map_err(
                    |err| anyhow::anyhow!("failed to parse `{input}` as `{FORMAT}`: {err}"),
                )?;
                let unix_time_ms: u64 = naive
                    .and_utc()
                    .timestamp_millis()
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("timestamp out of range"))?;
                Args::set_to(unix_time_ms)
            }
        };
        Ok(args.as_program_args()?)
    }
}

#[distributed_slice(dusk_program_sh::entry::SH_ENTRIES)]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: ProgramInfo {
            program_id: Some(date_capnp::PROGRAM_ID),
            name: "date",
            short_description: "show or set the current time",
            long_description: r#"
Show or set the current time.
* Use `date` to print the current time in the format "`YYYY-MM-DD HH:MM:SS` (UTC)".
* Use `date -s "YYYY-MM-DD HH:MM:SS"` to set the clock to that UTC timestamp.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(DateProgramArgsBuilder {}),
    }
}
