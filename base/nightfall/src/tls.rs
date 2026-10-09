use std::prelude::rust_2024::*;

use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use async_io::Async;
use dusk_program::embassy_time;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore, SupportedProtocolVersion};

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const KEEPALIVE_IDLE: Duration = Duration::from_secs(60);
pub(crate) const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
pub(crate) const KEEPALIVE_RETRIES: u32 = 3;
pub(crate) const USER_TIMEOUT: Duration = Duration::from_secs(60);
const MAXIMUM_TRUST_ANCHORS_BYTES: usize = 1024 * 1024;

pub(crate) type TlsStream = futures_rustls::client::TlsStream<Async<TcpStream>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostPort {
    pub(crate) host: String,
    pub(crate) port: u16,
}

impl HostPort {
    pub(crate) fn parse(text: &str) -> Result<HostPort, String> {
        let invalid = || format!("`{text}` is not <host>:<port>");
        let (host, port) = if let Some(rest) = text.strip_prefix('[') {
            let (host, rest) = rest.split_once(']').ok_or_else(invalid)?;
            if host.parse::<std::net::Ipv6Addr>().is_err() {
                return Err(format!("`{host}` in `{text}` is not an IPv6 address"));
            }
            (host, rest.strip_prefix(':').ok_or_else(invalid)?)
        } else {
            let (host, port) = text.rsplit_once(':').ok_or_else(invalid)?;
            if host.contains(':') {
                return Err(format!(
                    "`{text}` is ambiguous: write an IPv6 address as [<address>]:<port>"
                ));
            }
            (host, port)
        };
        if host.is_empty() || host.chars().any(|character| character.is_whitespace()) {
            return Err(invalid());
        }
        let port = port
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| format!("`{port}` in `{text}` is not a port from 1 to 65535"))?;
        Ok(HostPort {
            host: host.to_string(),
            port,
        })
    }
}

impl core::fmt::Display for HostPort {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.host.contains(':') {
            write!(formatter, "[{}]:{}", self.host, self.port)
        } else {
            write!(formatter, "{}:{}", self.host, self.port)
        }
    }
}

pub(crate) fn server_name(name: &str) -> Result<ServerName<'static>, String> {
    ServerName::try_from(name.to_string())
        .map_err(|error| format!("`{name}` is not a valid TLS server name: {error}"))
}

pub(crate) async fn trust_anchors(path: &str) -> anyhow::Result<RootCertStore> {
    let contents = crate::file::read(path, MAXIMUM_TRUST_ANCHORS_BYTES).await?;
    parse_trust_anchors(&contents).with_context(|| format!("`{path}` holds no usable trust anchor"))
}

fn parse_trust_anchors(contents: &[u8]) -> anyhow::Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    for certificate in CertificateDer::pem_slice_iter(contents) {
        roots.add(certificate?)?;
    }
    anyhow::ensure!(!roots.is_empty(), "no PEM CERTIFICATE block");
    Ok(roots)
}

struct SingleClientCertificate(Arc<rustls::sign::CertifiedKey>);

impl core::fmt::Debug for SingleClientCertificate {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("SingleClientCertificate")
            .field("chain_length", &self.0.cert.len())
            .finish()
    }
}

impl rustls::client::ResolvesClientCert for SingleClientCertificate {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        signature_schemes: &[rustls::SignatureScheme],
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        if self.0.key.choose_scheme(signature_schemes).is_none() {
            tracing::warn!(
                offered = ?signature_schemes,
                "the server accepts no signature scheme the node key can make"
            );
        }
        Some(self.0.clone())
    }

    fn has_certs(&self) -> bool {
        true
    }
}

pub(crate) fn client_config(
    roots: RootCertStore,
    versions: &[&'static SupportedProtocolVersion],
    client_certificate: Option<Arc<rustls::sign::CertifiedKey>>,
) -> anyhow::Result<Arc<ClientConfig>> {
    let builder =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(versions)
            .context("the TLS provider refuses the protocol versions")?
            .with_root_certificates(roots);
    let config = match client_certificate {
        Some(certified_key) => {
            builder.with_client_cert_resolver(Arc::new(SingleClientCertificate(certified_key)))
        }
        None => builder.with_no_client_auth(),
    };
    Ok(Arc::new(config))
}

pub(crate) async fn resolve(target: &HostPort) -> anyhow::Result<Vec<SocketAddr>> {
    if let Ok(address) = target.host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(address, target.port)]);
    }
    let (sender, receiver) = futures::channel::oneshot::channel();
    let host = target.host.clone();
    let port = target.port;
    std::thread::Builder::new()
        .name(String::from("nightfall-resolve"))
        .spawn(move || {
            let result = (host.as_str(), port)
                .to_socket_addrs()
                .map(|addresses| addresses.collect::<Vec<_>>());
            sender.send(result).ok();
        })
        .context("couldn't start a thread to resolve a host name")?;
    let addresses = embassy_time::with_timeout(embassy_duration(RESOLVE_TIMEOUT), receiver)
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "resolving {} took longer than {} s",
                target.host,
                RESOLVE_TIMEOUT.as_secs()
            )
        })?
        .map_err(|_| anyhow::anyhow!("the thread resolving {} died", target.host))?
        .with_context(|| format!("couldn't resolve {}", target.host))?;
    anyhow::ensure!(
        !addresses.is_empty(),
        "{} resolves to no address",
        target.host
    );
    Ok(addresses)
}

pub(crate) fn configure_socket(stream: &TcpStream, address: SocketAddr) {
    let unset = |option: &str, error: std::io::Error| {
        tracing::warn!(
            address = %address,
            option,
            error = %error,
            "the platform refused a socket option; keeping the connection without it"
        );
    };
    if let Err(error) = stream.set_nodelay(true) {
        unset("TCP_NODELAY", error);
    }
    let socket = socket2::SockRef::from(stream);
    let keepalive = socket2::TcpKeepalive::new().with_time(KEEPALIVE_IDLE);
    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "illumos",
        target_os = "ios",
        target_os = "visionos",
        target_os = "linux",
        target_os = "macos",
        target_os = "netbsd",
        target_os = "tvos",
        target_os = "watchos",
        target_os = "windows",
        target_os = "cygwin",
    ))]
    let keepalive = keepalive.with_interval(KEEPALIVE_INTERVAL);
    if let Err(error) = socket.set_tcp_keepalive(&keepalive) {
        unset("SO_KEEPALIVE", error);
    }
    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "illumos",
        target_os = "ios",
        target_os = "visionos",
        target_os = "linux",
        target_os = "macos",
        target_os = "netbsd",
        target_os = "tvos",
        target_os = "watchos",
        target_os = "windows",
        target_os = "cygwin",
    ))]
    if let Err(error) = socket.set_tcp_keepalive(&keepalive.with_retries(KEEPALIVE_RETRIES)) {
        unset("TCP_KEEPCNT", error);
    }
    #[cfg(any(
        target_os = "android",
        target_os = "fuchsia",
        target_os = "linux",
        target_os = "cygwin"
    ))]
    if let Err(error) = socket.set_tcp_user_timeout(Some(USER_TIMEOUT)) {
        unset("TCP_USER_TIMEOUT", error);
    }
}

pub(crate) async fn connect_tcp(target: &HostPort) -> anyhow::Result<Async<TcpStream>> {
    let addresses = resolve(target).await?;
    let mut failures = Vec::new();
    for address in addresses {
        let attempt = embassy_time::with_timeout(
            embassy_duration(CONNECT_TIMEOUT),
            Async::<TcpStream>::connect(address),
        )
        .await;
        match attempt {
            Ok(Ok(stream)) => {
                configure_socket(stream.get_ref(), address);
                return Ok(stream);
            }
            Ok(Err(error)) => failures.push(format!("{address}: {error}")),
            Err(_) => failures.push(format!(
                "{address}: no answer within {} s",
                CONNECT_TIMEOUT.as_secs()
            )),
        }
    }
    anyhow::bail!("couldn't connect to {target}: {}", failures.join("; "))
}

pub(crate) struct Connected {
    pub(crate) stream: TlsStream,
    pub(crate) socket: TcpStream,
    pub(crate) peer: SocketAddr,
}

pub(crate) async fn connect_tls(
    target: &HostPort,
    server_name: ServerName<'static>,
    config: Arc<ClientConfig>,
) -> anyhow::Result<Connected> {
    let tcp = connect_tcp(target).await?;
    let socket = tcp
        .get_ref()
        .try_clone()
        .context("couldn't duplicate the socket")?;
    let peer = tcp
        .get_ref()
        .peer_addr()
        .context("couldn't read the peer address")?;
    let connector = futures_rustls::TlsConnector::from(config);
    let stream = embassy_time::with_timeout(
        embassy_duration(HANDSHAKE_TIMEOUT),
        connector.connect(server_name, tcp),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "the TLS handshake with {target} took longer than {} s",
            HANDSHAKE_TIMEOUT.as_secs()
        )
    })?
    .with_context(|| format!("the TLS handshake with {target} failed"))?;
    Ok(Connected {
        stream,
        socket,
        peer,
    })
}

pub(crate) fn embassy_duration(duration: Duration) -> embassy_time::Duration {
    embassy_time::Duration::from_micros(u64::try_from(duration.as_micros()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_port_parses_names_and_addresses() {
        assert_eq!(
            HostPort::parse("fleet.example:443").unwrap(),
            HostPort {
                host: String::from("fleet.example"),
                port: 443
            }
        );
        assert_eq!(
            HostPort::parse("10.0.0.1:8443").unwrap(),
            HostPort {
                host: String::from("10.0.0.1"),
                port: 8443
            }
        );
        let ipv6 = HostPort::parse("[2001:db8::1]:443").unwrap();
        assert_eq!(ipv6.host, "2001:db8::1");
        assert_eq!(ipv6.to_string(), "[2001:db8::1]:443");
        assert_eq!(
            HostPort::parse("fleet.example:443").unwrap().to_string(),
            "fleet.example:443"
        );
    }

    #[test]
    fn host_port_rejects_malformed_input() {
        for text in [
            "",
            "fleet.example",
            ":443",
            "fleet.example:",
            "fleet.example:0",
            "fleet.example:65536",
            "fleet.example:https",
            "2001:db8::1:443",
            "[2001:db8::1]",
            "[2001:db8::1]443",
            "[fleet.example]:443",
            "fleet example:443",
        ] {
            assert!(HostPort::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn server_names_accept_dns_names_and_ip_literals() {
        assert!(server_name("fleet.example").is_ok());
        assert!(server_name("127.0.0.1").is_ok());
        assert!(server_name("::1").is_ok());
        assert!(server_name("not a name").is_err());
    }

    #[test]
    fn trust_anchors_need_at_least_one_certificate() {
        assert!(parse_trust_anchors(b"").is_err());
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut parameters = rcgen::CertificateParams::default();
        parameters.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let authority = parameters.self_signed(&key).unwrap();
        assert!(parse_trust_anchors(key.serialize_pem().as_bytes()).is_err());
        let both = format!("{}{}", key.serialize_pem(), authority.pem());
        assert_eq!(parse_trust_anchors(both.as_bytes()).unwrap().len(), 1);
        let corrupted = "-----BEGIN CERTIFICATE-----\nMAMCAQE=\n-----END CERTIFICATE-----\n";
        assert!(parse_trust_anchors(corrupted.as_bytes()).is_err());
    }

    #[test]
    fn keepalive_and_user_timeout_are_set() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stream = TcpStream::connect(address).unwrap();
        configure_socket(&stream, address);
        let socket = socket2::SockRef::from(&stream);
        assert!(socket.keepalive().unwrap());
        assert!(stream.nodelay().unwrap());
        #[cfg(target_os = "linux")]
        {
            assert_eq!(socket.tcp_keepalive_time().unwrap(), KEEPALIVE_IDLE);
            assert_eq!(socket.tcp_keepalive_interval().unwrap(), KEEPALIVE_INTERVAL);
            assert_eq!(socket.tcp_keepalive_retries().unwrap(), KEEPALIVE_RETRIES);
            assert_eq!(socket.tcp_user_timeout().unwrap(), Some(USER_TIMEOUT));
        }
    }
}
