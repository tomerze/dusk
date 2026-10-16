use crate::fleet::Fleet;
use crate::identity::{self, Identity, Issuer};
use crate::node::{Bootstrap, Node};
use capnp::message::ReaderOptions;
use capnp_rpc::rpc_twoparty_capnp::Side;
use capnp_rpc::{RpcSystem, twoparty};
use rustls_pki_types::ServerName;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::{Instant, sleep, sleep_until, timeout};
use tokio_rustls::TlsConnector;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
pub const LINK_BACKOFF_BASE: Duration = Duration::from_secs(1);
pub const LINK_BACKOFF_CAP: Duration = Duration::from_secs(300);
pub const LINK_STABLE_AFTER: Duration = Duration::from_secs(60);

pub struct Settings {
    pub fleet: Arc<Fleet>,
    pub server_name: ServerName<'static>,
    pub issuer: Issuer,
    pub simulator: u64,
    pub heartbeat_timeout: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    Closed,
    Failed,
    Silent,
}

impl Ended {
    pub fn reason(self) -> &'static str {
        match self {
            Ended::Closed => "closed",
            Ended::Failed => "failed",
            Ended::Silent => "silent",
        }
    }
}

pub fn backoff_ceiling(attempt: u32) -> Duration {
    let factor = 1u32.checked_shl(attempt.min(31)).unwrap_or(u32::MAX);
    LINK_BACKOFF_BASE
        .saturating_mul(factor)
        .min(LINK_BACKOFF_CAP)
}

fn backoff(attempt: u32) -> Duration {
    backoff_ceiling(attempt).mul_f64(rand::random::<f64>())
}

async fn silence(node: &Node, heartbeat_timeout: Duration) {
    loop {
        let deadline = Instant::from_std(node.last_heard()) + heartbeat_timeout;
        if Instant::now() >= deadline {
            return;
        }
        sleep_until(deadline).await;
    }
}

fn connect_failed(fleet: &Fleet) {
    metrics::counter!("nightfall_simulator_connect_failures_total").increment(1);
    fleet.connect_failed();
}

pub async fn serve(
    settings: &Settings,
    node: &Rc<Node>,
    identity: &Identity,
    link: u64,
) -> anyhow::Result<(Ended, Duration)> {
    let Some(address) = settings.fleet.address(link) else {
        connect_failed(&settings.fleet);
        anyhow::bail!("{} is not resolved yet", settings.fleet.name());
    };
    let tcp = match timeout(CONNECT_TIMEOUT, TcpStream::connect(address)).await {
        Ok(Ok(tcp)) => tcp,
        Ok(Err(error)) => {
            connect_failed(&settings.fleet);
            return Err(anyhow::Error::new(error).context(format!("connect to {address}")));
        }
        Err(_) => {
            connect_failed(&settings.fleet);
            anyhow::bail!(
                "no connection to {address} within {} s",
                CONNECT_TIMEOUT.as_secs()
            );
        }
    };
    tcp.set_nodelay(true)?;
    let connector = TlsConnector::from(identity.config.clone());
    let tls = match timeout(
        HANDSHAKE_TIMEOUT,
        connector.connect(settings.server_name.clone(), tcp),
    )
    .await
    {
        Ok(Ok(tls)) => tls,
        Ok(Err(error)) => {
            metrics::counter!("nightfall_simulator_handshakes_total", "outcome" => "failed")
                .increment(1);
            return Err(error.into());
        }
        Err(_) => {
            metrics::counter!("nightfall_simulator_handshakes_total", "outcome" => "failed")
                .increment(1);
            anyhow::bail!("no handshake within {} s", HANDSHAKE_TIMEOUT.as_secs());
        }
    };
    metrics::counter!("nightfall_simulator_handshakes_total", "outcome" => "completed")
        .increment(1);
    let (reader, writer) = tokio::io::split(tls);
    let network = twoparty::VatNetwork::new(
        reader.compat(),
        writer.compat_write(),
        Side::Server,
        ReaderOptions::new(),
    );
    let system = RpcSystem::new(
        Box::new(network),
        Some(Bootstrap::client(node.clone()).client),
    );
    node.heard();
    let linked_at = std::time::Instant::now();
    metrics::gauge!("nightfall_simulator_links_up").increment(1);
    tracing::debug!(device_id = %node.device_id, namespace_id = %format_args!("{:x}", node.namespace_id), %address, "linked to the fleet");
    let ended = tokio::select! {
        ended = system => match ended {
            Ok(()) => Ended::Closed,
            Err(error) => {
                tracing::debug!(device_id = %node.device_id, %error, "the link failed");
                Ended::Failed
            }
        },
        () = silence(node, settings.heartbeat_timeout) => Ended::Silent,
    };
    metrics::gauge!("nightfall_simulator_links_up").decrement(1);
    metrics::counter!("nightfall_simulator_links_lost_total", "reason" => ended.reason())
        .increment(1);
    let heard_for = node.last_heard().saturating_duration_since(linked_at);
    tracing::debug!(device_id = %node.device_id, reason = ended.reason(), heard_for_ms = heard_for.as_millis() as u64, "the fleet link is down");
    Ok((ended, heard_for))
}

pub async fn run(settings: Arc<Settings>, link: u64, start_at: Instant) {
    sleep_until(start_at).await;
    let device_id = identity::device_id(settings.simulator, link);
    let installation_id = identity::installation_id(&device_id);
    let node = Node::new(
        rand::random(),
        device_id,
        format!("simulated-{}-{link}", settings.simulator),
    );
    let mut identity: Option<Identity> = None;
    let mut attempt = 0u32;
    loop {
        if identity.as_ref().is_none_or(Identity::renewal_due) {
            match settings.issuer.mint(&node.device_id, &installation_id) {
                Ok(minted) => identity = Some(minted),
                Err(error) => {
                    tracing::error!(device_id = %node.device_id, error = %format_args!("{error:#}"), "couldn't mint the link's certificate");
                }
            }
        }
        if let Some(identity) = &identity {
            match serve(&settings, &node, identity, link).await {
                Ok((_, heard_for)) if heard_for > LINK_STABLE_AFTER => attempt = 0,
                Ok(_) => {}
                Err(error) => {
                    tracing::debug!(device_id = %node.device_id, error = %format_args!("{error:#}"), "couldn't link to the fleet");
                }
            }
        }
        let delay = backoff(attempt);
        attempt = attempt.saturating_add(1);
        sleep(delay).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::provider;
    use crate::identity::tests::Authority;
    use dusk_capnp::dusk_capnp::dusk;
    use rustls::server::WebPkiClientVerifier;
    use rustls::{RootCertStore, ServerConfig};
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;

    const SERVER_NAME: &str = "fleet.dusk.test";

    #[test]
    fn backs_off_like_the_node_up_to_its_cap() {
        let ceilings: Vec<u64> = (0..12)
            .map(|attempt| backoff_ceiling(attempt).as_secs())
            .collect();
        assert_eq!(ceilings, [1, 2, 4, 8, 16, 32, 64, 128, 256, 300, 300, 300]);
        assert_eq!(backoff_ceiling(u32::MAX), LINK_BACKOFF_CAP);
        for attempt in 0..40 {
            assert!(backoff(attempt) <= backoff_ceiling(attempt));
        }
    }

    fn acceptor(fleet_client: &Authority, fleet_server: &Authority) -> TlsAcceptor {
        let mut roots = RootCertStore::empty();
        roots.add(fleet_client.certificate.clone()).unwrap();
        let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider())
            .build()
            .unwrap();
        let (chain, key) = fleet_server.server(SERVER_NAME);
        let config = ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_client_cert_verifier(verifier)
            .with_single_cert(chain, key)
            .unwrap();
        TlsAcceptor::from(Arc::new(config))
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_link_serves_its_node_over_tls_and_closes_when_nightfall_falls_silent() {
        let fleet_client = Authority::new("fleet-client");
        let fleet_server = Authority::new("fleet-server");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fleet = Fleet::new(listener.local_addr().unwrap().to_string());
        fleet.resolve().await;
        let settings = Settings {
            fleet,
            server_name: ServerName::try_from(SERVER_NAME).unwrap(),
            issuer: fleet_client.issuer(&fleet_server, Duration::from_secs(3600)),
            simulator: 0,
            heartbeat_timeout: Duration::from_millis(300),
        };
        let device_id = identity::device_id(0, 1);
        let node = Node::new(0x5eed, device_id.clone(), "simulated-0-1".to_string());
        let identity = settings
            .issuer
            .mint(&device_id, &identity::installation_id(&device_id))
            .unwrap();
        let acceptor = acceptor(&fleet_client, &fleet_server);
        tokio::task::LocalSet::new()
            .run_until(async {
                let nightfall = tokio::task::spawn_local(async move {
                    let (tcp, _) = listener.accept().await.unwrap();
                    let tls = acceptor.accept(tcp).await.unwrap();
                    let presented = tls.get_ref().1.peer_certificates().unwrap().len();
                    let (reader, writer) = tokio::io::split(tls);
                    let network = twoparty::VatNetwork::new(
                        reader.compat(),
                        writer.compat_write(),
                        Side::Client,
                        ReaderOptions::new(),
                    );
                    let mut system = RpcSystem::new(Box::new(network), None);
                    let node: dusk::Client = system.bootstrap(Side::Server);
                    let served = tokio::task::spawn_local(system);
                    let namespace_id = node
                        .namespace_id_request()
                        .send()
                        .promise
                        .await
                        .unwrap()
                        .get()
                        .unwrap()
                        .get_result();
                    node.time_request().send().promise.await.unwrap();
                    served.await.unwrap_or_else(|error| panic!("{error}")).ok();
                    (presented, namespace_id)
                });
                let (ended, heard_for) = serve(&settings, &node, &identity, 1).await.unwrap();
                assert_eq!(ended, Ended::Silent);
                assert!(heard_for < Duration::from_secs(5), "{heard_for:?}");
                let (presented, namespace_id) = nightfall.await.unwrap();
                assert_eq!(presented, 2);
                assert_eq!(namespace_id, 0x5eed);
            })
            .await;
    }
}
