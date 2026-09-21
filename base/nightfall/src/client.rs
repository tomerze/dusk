use super::*;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::rc::Rc;

fn parse_port_or_socket_address(text: &str) -> Result<SocketAddr, String> {
    if let Ok(port) = text.parse::<u16>() {
        return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port));
    }
    text.parse::<SocketAddr>()
        .map_err(|_| format!("`{text}` is neither a port nor an ip:port"))
}

#[derive(clap::Parser)]
#[command(name = "nightfall", no_binary_name = true)]
struct NightfallCli {
    #[arg(
        short = 'l',
        long = "listen",
        value_parser = parse_port_or_socket_address,
        help = "Where to listen: a port on every address, or an ip:port"
    )]
    listen: SocketAddr,
}

struct NightfallProgramArgsBuilder {}

#[dusk_program::async_trait::async_trait(?Send)]
impl ProgramArgsBuilder for NightfallProgramArgsBuilder {
    async fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = NightfallCli::try_parse_from(args)?;
        Ok(Args::new(&cli.listen.ip().to_string(), cli.listen.port()).as_program_args()?)
    }
}

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(nightfall_capnp::PROGRAM_ID),
            name: "nightfall",
            short_description: "accept Dusk sessions over TCP",
            long_description: r#"
The `nightfall` program listens on TCP and opens a Dusk session with every connection it accepts.
It keeps running until it is terminated.
* Use `nightfall -l <port>` to listen on `<port>` on every address.
* Use `nightfall -l <ip>:<port>` to listen on one address.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(NightfallProgramArgsBuilder {}),
    }
}
