extern crate linkme;

use super::*;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{ProgramArgsBuilder, ProgramInfo, ShEntry};
use linkme::distributed_slice;
use std::net::{SocketAddr, ToSocketAddrs};
use std::rc::Rc;

const FORMAT: &str = "%Y-%m-%d %H:%M:%S";

#[derive(clap::Parser)]
#[command(name = "date", no_binary_name = true)]
struct DateCli {
    /// Set the clock to the given UTC timestamp in `YYYY-MM-DD HH:MM:SS` form.
    #[arg(short = 's', long = "set", group = "action")]
    set: Option<String>,
    /// Query the given NTP server (`host[:port]`, default port 123) and set
    /// the server clock to the time it reports.
    #[arg(
        long = "ntp",
        group = "action",
        value_name = "HOST[:PORT]",
        value_parser = parse_ntp_server,
    )]
    ntp: Option<SocketAddr>,
}

fn parse_ntp_server(input: &str) -> Result<SocketAddr, String> {
    let with_port = if input.contains(':') {
        input.to_string()
    } else {
        format!("{input}:123")
    };
    with_port
        .to_socket_addrs()
        .map_err(|err| format!("failed to resolve `{input}`: {err}"))?
        .next()
        .ok_or_else(|| format!("no addresses resolved for `{input}`"))
}

struct DateProgramArgsBuilder {}

impl ProgramArgsBuilder for DateProgramArgsBuilder {
    fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = DateCli::try_parse_from(args)?;
        let args = match (cli.set, cli.ntp) {
            (Some(set_to), _) => {
                let naive = chrono::NaiveDateTime::parse_from_str(&set_to, FORMAT).map_err(
                    |err| anyhow::anyhow!("failed to parse `{set_to}` as `{FORMAT}`: {err}"),
                )?;
                let unix_time_ms: u64 = naive
                    .and_utc()
                    .timestamp_millis()
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("timestamp out of range"))?;
                Args::set_to(unix_time_ms)
            }
            (_, Some(server)) => Args::set_to(query_ntp(server)?),
            _ => Args::show(),
        };
        Ok(args.as_program_args()?)
    }
}

fn query_ntp(server: SocketAddr) -> anyhow::Result<u64> {
    use sntpc::{NtpContext, StdTimestampGen, sync::get_time};
    use sntpc_net_std::UdpSocketWrapper;
    use std::net::UdpSocket;
    use std::time::Duration;

    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    socket.set_write_timeout(Some(Duration::from_secs(5)))?;
    let socket = UdpSocketWrapper::new(socket);
    let context = NtpContext::new(StdTimestampGen::default());

    let result = get_time(server, &socket, context)
        .map_err(|err| anyhow::anyhow!("NTP query to `{server}` failed: {err:?}"))?;
    Ok(result.sec() as u64 * 1000 + (result.sec_fraction() as u64 * 1000) / (1u64 << 32))
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
* Use `date --ntp <HOST[:PORT]>` (for example `pool.ntp.org`) to query an NTP server and set the clock to its reported time. Port defaults to 123.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(DateProgramArgsBuilder {}),
    }
}
