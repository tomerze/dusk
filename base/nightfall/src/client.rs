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

fn parse_host_port(text: &str) -> Result<String, String> {
    tls::HostPort::parse(text).map(|_| text.to_string())
}

#[derive(clap::Parser)]
#[command(name = "nightfall", no_binary_name = true)]
#[command(group(clap::ArgGroup::new("mode").required(true).args(["listen", "connect"])))]
struct NightfallCli {
    #[arg(
        short = 'l',
        long = "listen",
        value_parser = parse_port_or_socket_address,
        help = "Where to listen: a port on every address, or an ip:port"
    )]
    listen: Option<SocketAddr>,
    #[arg(
        short = 'c',
        long = "connect",
        value_name = "HOST:PORT",
        value_parser = parse_host_port,
        requires_all = ["provision", "ca"],
        help = "Dial out to nightfall at <host>:<port> and serve the node over that link"
    )]
    connect: Option<String>,
    #[arg(
        long,
        value_name = "NAME",
        requires = "connect",
        help = "The TLS server name of --connect, when it is not its host"
    )]
    server_name: Option<String>,
    #[arg(
        long,
        value_name = "HOST:PORT",
        value_parser = parse_host_port,
        requires = "connect",
        help = "Where to get and renew the node's certificate"
    )]
    provision: Option<String>,
    #[arg(
        long,
        value_name = "NAME",
        requires = "connect",
        help = "The TLS server name of --provision, when it is not its host"
    )]
    provision_server_name: Option<String>,
    #[arg(
        long,
        value_name = "PATH",
        requires = "connect",
        help = "PEM file of the certificate authorities that sign nightfall's server certificates"
    )]
    ca: Option<String>,
    #[arg(
        long,
        value_name = "PATH",
        requires = "connect",
        help = "A file holding an install token to enroll with instead of the fleet token"
    )]
    install_token_file: Option<String>,
    #[arg(
        long,
        value_name = "SECONDS",
        requires = "connect",
        default_value_t = connect::DEFAULT_HEARTBEAT_TIMEOUT_SECONDS,
        value_parser = clap::value_parser!(u32).range(1..=86400),
        help = "Close the link when nothing arrived on it for this long"
    )]
    heartbeat_timeout: u32,
}

impl NightfallCli {
    fn into_args(self) -> anyhow::Result<Args> {
        if let Some(listen) = self.listen {
            return Ok(Args::new(&listen.ip().to_string(), listen.port()));
        }
        let missing = |name: &str| anyhow::anyhow!("--connect needs {name}");
        Ok(Args::connect(&connect::ConnectArgs {
            fleet: self.connect.ok_or_else(|| missing("<host>:<port>"))?,
            fleet_server_name: self.server_name,
            provision: self.provision.ok_or_else(|| missing("--provision"))?,
            provision_server_name: self.provision_server_name,
            trust_anchors: self.ca.ok_or_else(|| missing("--ca"))?,
            install_token_file: self.install_token_file,
            heartbeat_timeout_seconds: self.heartbeat_timeout,
        }))
    }
}

struct NightfallProgramArgsBuilder {}

#[dusk_program::async_trait::async_trait(?Send)]
impl ProgramArgsBuilder for NightfallProgramArgsBuilder {
    async fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = NightfallCli::try_parse_from(args)?;
        Ok(cli.into_args()?.as_program_args()?)
    }
}

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(nightfall_capnp::PROGRAM_ID),
            name: "nightfall",
            short_description: "accept Dusk sessions over TCP, or dial out to nightfall",
            long_description: r#"
The `nightfall` program serves the node's Dusk sessions. It keeps running until it is terminated.
* Use `nightfall -l <port>` to listen on `<port>` on every address.
* Use `nightfall -l <ip>:<port>` to listen on one address.
* Use `nightfall -c <host>:<port> --provision <host>:<port> --ca <file>` to dial out to nightfall
  instead, over TLS 1.3 with the node's certificate. The first time, the node gets its certificate
  from `--provision` with the fleet token it was built with, or with the install token in
  `--install-token-file <file>`. It keeps its identity in persistent kvs keys, so it needs a node
  built with a persistent kvs file, and renews its certificate before it expires.
  `--server-name` and `--provision-server-name` set the TLS server names when they are not the
  hosts, and `--heartbeat-timeout <seconds>` (90 by default) closes a link that has gone silent.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(NightfallProgramArgsBuilder {}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<NightfallCli, clap::Error> {
        NightfallCli::try_parse_from(line.split_whitespace())
    }

    const CONNECT: &str =
        "-c fleet.example:443 --provision provision.example:443 --ca /etc/dusk/fleet-server-ca.pem";

    fn connect_arguments(line: &str) -> connect::ConnectArgs {
        let program_args = parse(line)
            .unwrap()
            .into_args()
            .unwrap()
            .as_program_args()
            .unwrap();
        program_args
            .with_data::<nightfall_capnp::nightfall_args::data::Owned, _, _>(|data| {
                assert!(data.has_connect());
                read_connect(data.get_connect()?)
            })
            .unwrap()
    }

    #[test]
    fn listen_mode_still_parses() {
        let cli = parse("-l 9090").unwrap();
        assert_eq!(cli.listen.unwrap().port(), 9090);
        assert!(cli.connect.is_none());
        let program_args = cli.into_args().unwrap().as_program_args().unwrap();
        program_args
            .with_data::<nightfall_capnp::nightfall_args::data::Owned, _, _>(|data| {
                assert!(!data.has_connect());
                assert_eq!(data.get_port(), 9090);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn connect_mode_parses_every_option() {
        let cli = parse(&format!(
            "{CONNECT} --server-name fleet.dusk.example --provision-server-name provision.dusk.example --install-token-file /etc/dusk/install-token --heartbeat-timeout 30"
        ))
        .unwrap();
        assert_eq!(cli.connect.as_deref(), Some("fleet.example:443"));
        assert_eq!(cli.server_name.as_deref(), Some("fleet.dusk.example"));
        assert_eq!(cli.provision.as_deref(), Some("provision.example:443"));
        assert_eq!(
            cli.provision_server_name.as_deref(),
            Some("provision.dusk.example")
        );
        assert_eq!(cli.ca.as_deref(), Some("/etc/dusk/fleet-server-ca.pem"));
        assert_eq!(
            cli.install_token_file.as_deref(),
            Some("/etc/dusk/install-token")
        );
        assert_eq!(cli.heartbeat_timeout, 30);
        assert_eq!(parse(CONNECT).unwrap().heartbeat_timeout, 90);
    }

    #[test]
    fn connect_mode_refuses_incomplete_or_conflicting_options() {
        for line in [
            "",
            "-c fleet.example:443",
            "-c fleet.example:443 --provision provision.example:443",
            "-c fleet.example:443 --ca ca.pem",
            "-c fleet.example --provision provision.example:443 --ca ca.pem",
            "-c fleet.example:443 --provision provision.example --ca ca.pem",
            "-l 9090 -c fleet.example:443 --provision provision.example:443 --ca ca.pem",
            "-l 9090 --install-token-file install",
            "-l 9090 --ca ca.pem",
            "-l 9090 --heartbeat-timeout 30",
            "-c fleet.example:443 --provision provision.example:443 --ca ca.pem --token t",
            "-c fleet.example:443 --provision provision.example:443 --ca ca.pem --token-file t",
            "-c fleet.example:443 --provision provision.example:443 --ca ca.pem --state /var/lib/dusk",
        ] {
            assert!(parse(line).is_err(), "{line}");
        }
        assert!(parse(&format!("{CONNECT} --heartbeat-timeout 0")).is_err());
    }

    #[test]
    fn connect_arguments_survive_the_args_message() {
        assert_eq!(
            connect_arguments(&format!("{CONNECT} --server-name fleet.dusk.example")),
            connect::ConnectArgs {
                fleet: String::from("fleet.example:443"),
                fleet_server_name: Some(String::from("fleet.dusk.example")),
                provision: String::from("provision.example:443"),
                provision_server_name: None,
                trust_anchors: String::from("/etc/dusk/fleet-server-ca.pem"),
                install_token_file: None,
                heartbeat_timeout_seconds: 90,
            }
        );
        assert_eq!(
            connect_arguments(&format!(
                "{CONNECT} --install-token-file /etc/dusk/install-token"
            ))
            .install_token_file
            .as_deref(),
            Some("/etc/dusk/install-token")
        );
    }
}
