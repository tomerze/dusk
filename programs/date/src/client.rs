use super::*;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
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
    /// Read the wall clock on the client and set the server clock to match it.
    #[arg(long = "sync", group = "action")]
    sync: bool,
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
        let args = match (cli.set, cli.ntp, cli.sync) {
            (Some(set_to), _, _) => {
                let naive =
                    chrono::NaiveDateTime::parse_from_str(&set_to, FORMAT).map_err(|err| {
                        anyhow::anyhow!("failed to parse `{set_to}` as `{FORMAT}`: {err}")
                    })?;
                let unix_time_ms: u64 = naive
                    .and_utc()
                    .timestamp_millis()
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("timestamp out of range"))?;
                Args::set_to(unix_time_ms)
            }
            (_, Some(server), _) => Args::set_to(query_ntp(server)?),
            (_, _, true) => Args::set_to(client_unix_time_ms()?),
            _ => Args::show(),
        };
        Ok(args.as_program_args()?)
    }
}

fn client_unix_time_ms() -> anyhow::Result<u64> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| anyhow::anyhow!("client clock is before the Unix epoch: {err}"))?;
    u64::try_from(elapsed.as_millis())
        .map_err(|_| anyhow::anyhow!("client unix time in ms does not fit in u64"))
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

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(date_capnp::PROGRAM_ID),
            name: "date",
            short_description: "show or set the current time",
            long_description: r#"
Show or set the current time.
* Use `date` to print the current time in the format "`YYYY-MM-DD HH:MM:SS` (UTC)".
* Use `date -s "YYYY-MM-DD HH:MM:SS"` to set the clock to that UTC timestamp.
* Use `date --ntp <HOST[:PORT]>` (for example `pool.ntp.org`) to query an NTP server and set the clock to its reported time. Port defaults to 123.
* Use `date --sync` to copy the wall clock from the client and set the server clock to match it.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(DateProgramArgsBuilder {}),
    }
}
