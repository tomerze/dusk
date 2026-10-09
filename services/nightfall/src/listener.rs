use crate::directory::{Route, parse_namespace};
use crate::limits::Failure;
use crate::proxy::Header;
use crate::shard::{Handoff, Shard, ShardListeners};
use crate::sni::{PeekFailure, UNRECOGNIZED_NAME_ALERT, peek_server_name};
use rustls::ServerConfig;
use std::cell::Cell;
use std::net::SocketAddr;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::server::TlsStream;

pub const ACCEPT_PAUSE: Duration = Duration::from_millis(100);
pub const RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const KEEPALIVE_IDLE: Duration = Duration::from_secs(60);
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
pub const KEEPALIVE_PROBES: u32 = 3;
pub const USER_TIMEOUT: Duration = Duration::from_secs(60);
pub const PROVISIONING_MAX_MESSAGE_BYTES: u64 = 64 * 1024;
pub const PROVISIONING_IDLE: Duration = Duration::from_secs(30);

struct Watched<Stream> {
    stream: Stream,
    activity: Rc<Cell<tokio::time::Instant>>,
}

impl<Stream: AsyncRead + Unpin> AsyncRead for Watched<Stream> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buffer.filled().len();
        let polled = Pin::new(&mut self.stream).poll_read(context, buffer);
        if buffer.filled().len() > before {
            self.activity.set(tokio::time::Instant::now());
        }
        polled
    }
}

impl<Stream: AsyncWrite + Unpin> AsyncWrite for Watched<Stream> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let polled = Pin::new(&mut self.stream).poll_write(context, bytes);
        if matches!(polled, Poll::Ready(Ok(written)) if written > 0) {
            self.activity.set(tokio::time::Instant::now());
        }
        polled
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(context)
    }
}

async fn idle(activity: &Cell<tokio::time::Instant>, limit: Duration) {
    loop {
        let deadline = activity.get() + limit;
        if tokio::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep_until(deadline).await;
    }
}

pub fn keep_alive(stream: &TcpStream) -> std::io::Result<()> {
    let socket = socket2::SockRef::from(stream);
    socket.set_tcp_keepalive(
        &socket2::TcpKeepalive::new()
            .with_time(KEEPALIVE_IDLE)
            .with_interval(KEEPALIVE_INTERVAL)
            .with_retries(KEEPALIVE_PROBES),
    )?;
    socket.set_tcp_user_timeout(Some(USER_TIMEOUT))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listener {
    Fleet,
    Provision,
    Inner,
    Relay,
}

impl Listener {
    pub fn name(self) -> &'static str {
        match self {
            Listener::Fleet => "fleet",
            Listener::Provision => "provision",
            Listener::Inner => "inner",
            Listener::Relay => "relay",
        }
    }
}

fn handshake(listener: Listener, outcome: &'static str) {
    metrics::counter!(
        "nightfall_handshakes_total",
        "listener" => listener.name(),
        "outcome" => outcome
    )
    .increment(1);
}

pub fn namespace_of(server_name: &str, suffix: &str) -> Option<u64> {
    let label = server_name.strip_suffix(suffix)?.strip_suffix('.')?;
    parse_namespace(label)
}

fn is_descriptor_exhaustion(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(code) if code == libc::EMFILE || code == libc::ENFILE)
}

async fn accept_loop(shard: Rc<Shard>, listener: TcpListener, kind: Listener) {
    let stop = match kind {
        Listener::Fleet | Listener::Provision => shard.shared.accepting.clone(),
        Listener::Inner | Listener::Relay => shard.shared.accepting_inner.clone(),
    };
    loop {
        let accepted = tokio::select! {
            () = stop.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        match accepted {
            Ok((stream, remote)) => {
                if let Err(error) = stream.set_nodelay(true) {
                    tracing::debug!(%remote, %error, "TCP_NODELAY could not be set");
                }
                if let Err(error) = keep_alive(&stream) {
                    tracing::warn!(%remote, %error, "TCP keepalive could not be set");
                }
                let shard = shard.clone();
                tokio::task::spawn_local(async move {
                    match kind {
                        Listener::Fleet | Listener::Provision => {
                            accept_fleet(shard, stream, remote, kind).await
                        }
                        Listener::Inner => accept_inner(shard, stream, remote).await,
                        Listener::Relay => accept_relay(shard, stream, remote).await,
                    }
                });
            }
            Err(error) if is_descriptor_exhaustion(&error) => {
                metrics::counter!("nightfall_accept_descriptor_exhaustion_total").increment(1);
                tracing::warn!(listener = kind.name(), %error, "out of file descriptors; pausing accepts");
                tokio::time::sleep(ACCEPT_PAUSE).await;
            }
            Err(error) => {
                tracing::warn!(listener = kind.name(), %error, "accepting a connection failed");
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
    }
    tracing::info!(
        shard = shard.index,
        listener = kind.name(),
        "stopped accepting"
    );
}

pub fn spawn(shard: &Rc<Shard>, listeners: ShardListeners) -> anyhow::Result<()> {
    let mut all = vec![
        (listeners.fleet, Listener::Fleet),
        (listeners.inner, Listener::Inner),
        (listeners.relay, Listener::Relay),
    ];
    if let Some(provision) = listeners.provision {
        all.push((provision, Listener::Provision));
    }
    for (listener, kind) in all {
        listener.set_nonblocking(true)?;
        let listener = TcpListener::from_std(listener)?;
        tokio::task::spawn_local(accept_loop(shard.clone(), listener, kind));
    }
    Ok(())
}

fn handshake_timeout(shard: &Shard) -> Duration {
    Duration::from_millis(shard.shared.config.fleet.handshake_timeout_ms)
}

fn admitted(shard: &Shard, remote: SocketAddr, kind: Listener) -> bool {
    if shard.shared.penalty_box.is_penalized(remote.ip()) {
        handshake(kind, "penalized");
        tracing::debug!(%remote, listener = kind.name(), "refused an address in the penalty box");
        return false;
    }
    if !shard.shared.handshakes.try_take() {
        handshake(kind, "rate_limited");
        metrics::counter!("nightfall_rate_limited_total", "limit" => "handshakes_per_second")
            .increment(1);
        tracing::debug!(%remote, listener = kind.name(), "refused a handshake above the instance rate");
        return false;
    }
    true
}

async fn peek(
    shard: &Shard,
    stream: &TcpStream,
    remote: SocketAddr,
    kind: Listener,
) -> Option<Option<String>> {
    match peek_server_name(stream, handshake_timeout(shard)).await {
        Ok(server_name) => Some(server_name),
        Err(failure) => {
            handshake(kind, "no_client_hello");
            match failure {
                PeekFailure::Closed => {
                    tracing::debug!(%remote, listener = kind.name(), %failure, "a connection closed before TLS")
                }
                _ => {
                    shard
                        .shared
                        .penalty_box
                        .failed(remote.ip(), Failure::Handshake);
                    tracing::debug!(%remote, listener = kind.name(), %failure, "a connection sent no usable ClientHello");
                }
            }
            None
        }
    }
}

async fn unrecognized(
    mut stream: TcpStream,
    remote: SocketAddr,
    kind: Listener,
    server_name: Option<&str>,
) {
    handshake(kind, "unknown_server_name");
    metrics::counter!("nightfall_unknown_server_names_total", "listener" => kind.name())
        .increment(1);
    tracing::debug!(%remote, listener = kind.name(), server_name, "refused an unknown server name");
    if let Err(error) = stream.write_all(&UNRECOGNIZED_NAME_ALERT).await {
        tracing::debug!(%remote, %error, "the unrecognized_name alert was not sent");
    }
    if let Err(error) = stream.shutdown().await {
        tracing::debug!(%remote, %error, "closing a refused connection failed");
    }
}

async fn tls_accept(
    shard: &Shard,
    stream: TcpStream,
    remote: SocketAddr,
    kind: Listener,
    config: Arc<ServerConfig>,
    expected_name: &str,
) -> Option<TlsStream<TcpStream>> {
    let accepting = async {
        let start =
            tokio_rustls::LazyConfigAcceptor::new(rustls::server::Acceptor::default(), stream)
                .await?;
        let offered = start
            .client_hello()
            .server_name()
            .map(str::to_ascii_lowercase);
        if offered.as_deref() != Some(expected_name) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "rustls read server name {offered:?} where the peek read {expected_name:?}"
                ),
            ));
        }
        start.into_stream(config).await
    };
    match tokio::time::timeout(handshake_timeout(shard), accepting).await {
        Ok(Ok(tls)) => {
            handshake(kind, "ok");
            Some(tls)
        }
        Ok(Err(error)) => {
            handshake(kind, "failed");
            shard
                .shared
                .penalty_box
                .failed(remote.ip(), Failure::Handshake);
            tracing::debug!(%remote, listener = kind.name(), %error, "a TLS handshake failed");
            None
        }
        Err(_) => {
            handshake(kind, "timed_out");
            shard
                .shared
                .penalty_box
                .failed(remote.ip(), Failure::Handshake);
            tracing::debug!(%remote, listener = kind.name(), "a TLS handshake timed out");
            None
        }
    }
}

async fn proxied_remote(
    shard: &Shard,
    stream: &mut TcpStream,
    peer: SocketAddr,
) -> Option<SocketAddr> {
    let fleet = &shard.shared.config.fleet;
    if !fleet.proxy_protocol {
        return Some(peer);
    }
    let trusted = fleet
        .proxy_protocol_trusted_cidrs
        .iter()
        .filter_map(|cidr| crate::limits::Cidr::parse(cidr).ok())
        .any(|cidr| cidr.contains(peer.ip()));
    if !trusted {
        tracing::warn!(%peer, "refused a fleet connection from an address that may not send a PROXY header");
        return None;
    }
    match crate::proxy::read_header(stream, handshake_timeout(shard)).await {
        Ok(Header::Proxied { source, .. }) => Some(source),
        Ok(Header::Local) => Some(peer),
        Err(failure) => {
            tracing::warn!(%peer, %failure, "a fleet connection sent no valid PROXY header");
            None
        }
    }
}

fn peer_certificates(tls: &TlsStream<TcpStream>) -> Vec<rustls_pki_types::CertificateDer<'static>> {
    tls.get_ref()
        .1
        .peer_certificates()
        .map(|certificates| {
            certificates
                .iter()
                .map(|certificate| certificate.clone().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

async fn accept_fleet(shard: Rc<Shard>, mut stream: TcpStream, peer: SocketAddr, kind: Listener) {
    let Some(remote) = proxied_remote(&shard, &mut stream, peer).await else {
        return;
    };
    if !admitted(&shard, remote, kind) {
        return;
    }
    let Some(server_name) = peek(&shard, &stream, remote, kind).await else {
        return;
    };
    let config = &shard.shared.config;
    let name = server_name.as_deref().unwrap_or_default();
    let fleet_name = config.fleet.server_names.iter().any(|known| known == name);
    let provision_name = (kind == Listener::Provision || config.shares_fleet_port())
        && config
            .provision
            .server_names
            .iter()
            .any(|known| known == name);
    if fleet_name && kind == Listener::Fleet {
        if !shard.shared.readiness.node_state.load(Ordering::Acquire) {
            handshake(Listener::Fleet, "not_ready");
            tracing::debug!(%remote, "refused a node before the node states are read");
            return;
        }
        let Some(counted) = crate::session::Counted::take(&shard.shared) else {
            handshake(Listener::Fleet, "full");
            metrics::counter!("nightfall_rate_limited_total", "limit" => "max_sessions")
                .increment(1);
            tracing::warn!(%remote, max_sessions = shard.shared.max_sessions, "refused a node: the instance holds its maximum of sessions");
            return;
        };
        let Some(tls) = tls_accept(
            &shard,
            stream,
            remote,
            Listener::Fleet,
            shard.shared.tls.fleet.clone(),
            name,
        )
        .await
        else {
            return;
        };
        let certificates = peer_certificates(&tls);
        let node = match certificates.first().map(crate::peer::node_certificate) {
            Some(Ok(node)) => node,
            Some(Err(error)) => {
                metrics::counter!("nightfall_sessions_refused_total", "reason" => "invalid_certificate")
                    .increment(1);
                shard
                    .shared
                    .penalty_box
                    .failed(remote.ip(), Failure::Credential);
                tracing::warn!(%remote, error = format!("{error:#}"), "refused a node certificate without a node identity");
                return;
            }
            None => {
                tracing::warn!(%remote, "a node presented no certificate");
                return;
            }
        };
        crate::session::serve_node(shard.clone(), tls, remote, node, counted).await;
    } else if provision_name {
        if !shard.shared.readiness.node_state.load(Ordering::Acquire) {
            handshake(Listener::Provision, "not_ready");
            tracing::debug!(%remote, "refused a provisioning client before the node states are read");
            return;
        }
        let Ok(_connection) = shard
            .shared
            .provisioning_connections
            .clone()
            .try_acquire_owned()
        else {
            handshake(Listener::Provision, "full");
            metrics::counter!("nightfall_rate_limited_total", "limit" => "max_provisioning_connections")
                .increment(1);
            tracing::warn!(%remote, "refused a provisioning client: max_provisioning_connections are open");
            return;
        };
        let Some(_address) = shard.shared.provisioning_addresses.try_take(remote.ip()) else {
            handshake(Listener::Provision, "full");
            metrics::counter!("nightfall_rate_limited_total", "limit" => "max_provisioning_connections_per_ip")
                .increment(1);
            tracing::debug!(%remote, "refused a provisioning client: its address holds max_provisioning_connections_per_ip");
            return;
        };
        let Some(tls) = tls_accept(
            &shard,
            stream,
            remote,
            Listener::Provision,
            shard.shared.tls.provision.clone(),
            name,
        )
        .await
        else {
            return;
        };
        serve_provisioning(shard, tls, remote).await;
    } else {
        unrecognized(stream, remote, kind, server_name.as_deref()).await;
    }
}

async fn serve_provisioning(shard: Rc<Shard>, tls: TlsStream<TcpStream>, remote: SocketAddr) {
    let connection = nightfall_provisioning::server::ConnectionInfo {
        remote_address: remote,
        peer_certificates: peer_certificates(&tls),
    };
    let client = shard.shared.provisioning.client(connection);
    let activity = Rc::new(Cell::new(tokio::time::Instant::now()));
    let audit = shard.audit.clone();
    let system = crate::rpc::system(
        Watched {
            stream: tls,
            activity: activity.clone(),
        },
        capnp_rpc::rpc_twoparty_capnp::Side::Server,
        PROVISIONING_MAX_MESSAGE_BYTES,
        Some(client.client),
        move |kind: &str| {
            tracing::error!(%remote, kind, "a provisioning client sent a rejected rpc message; the link is closed");
            let mut content = nightfall_ledger::entry::EntryContent::event(
                nightfall_ledger::entry::Event::RpcRejected,
                nightfall_ledger::entry::NIGHTFALL_PRINCIPAL,
            );
            content.event_detail = Some(crate::audit::detail(&[
                ("kind", serde_json::Value::String(kind.to_string())),
                (
                    "listener",
                    serde_json::Value::String("provision".to_string()),
                ),
                (
                    "remote_address",
                    serde_json::Value::String(remote.to_string()),
                ),
            ]));
            audit.record_event(content);
        },
    );
    let lifetime = Duration::from_millis(shard.shared.config.provision.challenge_ttl_ms);
    tokio::select! {
        outcome = tokio::time::timeout(lifetime, system) => match outcome {
            Ok(Ok(())) => tracing::debug!(%remote, "a provisioning client disconnected"),
            Ok(Err(error)) => tracing::debug!(%remote, %error, "a provisioning link failed"),
            Err(_) => {
                tracing::info!(%remote, "closed a provisioning link that outlived the challenge lifetime")
            }
        },
        () = idle(&activity, PROVISIONING_IDLE) => {
            tracing::info!(%remote, idle_seconds = PROVISIONING_IDLE.as_secs(), "closed an idle provisioning link")
        }
    }
}

async fn accept_inner(shard: Rc<Shard>, stream: TcpStream, remote: SocketAddr) {
    if !admitted(&shard, remote, Listener::Inner) {
        return;
    }
    route_inner(shard, stream, remote, false).await;
}

async fn accept_relay(shard: Rc<Shard>, mut stream: TcpStream, peer: SocketAddr) {
    let remote = match crate::proxy::read_header(&mut stream, handshake_timeout(&shard)).await {
        Ok(Header::Proxied { source, .. }) => source,
        Ok(Header::Local) => peer,
        Err(failure) => {
            handshake(Listener::Relay, "no_proxy_header");
            tracing::warn!(%peer, %failure, "a relayed connection sent no valid PROXY header");
            return;
        }
    };
    if !admitted(&shard, remote, Listener::Relay) {
        return;
    }
    route_inner(shard, stream, remote, true).await;
}

async fn route_inner(shard: Rc<Shard>, stream: TcpStream, remote: SocketAddr, relayed: bool) {
    let kind = if relayed {
        Listener::Relay
    } else {
        Listener::Inner
    };
    let Some(server_name) = peek(&shard, &stream, remote, kind).await else {
        return;
    };
    let suffix = &shard.shared.config.inner.server_name_suffix;
    let Some(namespace_id) = server_name
        .as_deref()
        .and_then(|name| namespace_of(name, suffix))
    else {
        unrecognized(stream, remote, kind, server_name.as_deref()).await;
        return;
    };
    let route = shard.shared.directory().route(namespace_id);
    match route {
        Route::Local { shard: owner, .. } if owner == shard.index => {
            serve_local(shard, stream, namespace_id, remote).await;
        }
        Route::Local { shard: owner, .. } => {
            hand_off(&shard, owner, stream, namespace_id, remote, relayed)
        }
        Route::Remote {
            instance,
            relay_address,
        } if !relayed => {
            relay(
                shard,
                stream,
                remote,
                namespace_id,
                &instance,
                &relay_address,
            )
            .await;
        }
        Route::Remote { .. } | Route::Nowhere => {
            serve_nowhere(shard, stream, namespace_id, remote, kind).await;
        }
    }
}

fn hand_off(
    shard: &Shard,
    owner: usize,
    stream: TcpStream,
    namespace_id: u64,
    remote: SocketAddr,
    relayed: bool,
) {
    let stream = match stream.into_std() {
        Ok(stream) => stream,
        Err(error) => {
            tracing::warn!(%remote, %error, "a connection could not be detached for another shard");
            return;
        }
    };
    let Some(handle) = shard.shared.shards.get(owner) else {
        tracing::error!(owner, "the directory names a shard that does not exist");
        return;
    };
    let handoff = Handoff {
        stream,
        namespace_id,
        remote,
        relayed,
    };
    match handle.handoff.try_send(handoff) {
        Ok(()) => {
            metrics::counter!("nightfall_shard_handoffs_total").increment(1);
            tracing::debug!(from = shard.index, to = owner, namespace_id = %crate::directory::namespace_hex(namespace_id), "handed an inner connection to the node's shard");
        }
        Err(error) => {
            metrics::counter!("nightfall_shard_handoff_failures_total").increment(1);
            tracing::warn!(from = shard.index, to = owner, %remote, %error, "an inner connection could not be handed to the node's shard");
        }
    }
}

pub fn take_handoff(shard: &Rc<Shard>, handoff: Handoff) {
    let Handoff {
        stream,
        namespace_id,
        remote,
        relayed,
    } = handoff;
    let stream = match stream
        .set_nonblocking(true)
        .and_then(|()| TcpStream::from_std(stream))
    {
        Ok(stream) => stream,
        Err(error) => {
            tracing::warn!(%remote, %error, "a handed connection could not join this shard");
            return;
        }
    };
    let shard = shard.clone();
    let kind = if relayed {
        Listener::Relay
    } else {
        Listener::Inner
    };
    tokio::task::spawn_local(async move {
        if shard.session(namespace_id).is_some() {
            serve_local(shard, stream, namespace_id, remote).await;
        } else {
            serve_nowhere(shard, stream, namespace_id, remote, kind).await;
        }
    });
}

async fn serve_local(shard: Rc<Shard>, stream: TcpStream, namespace_id: u64, remote: SocketAddr) {
    let name = format!(
        "{}.{}",
        crate::directory::namespace_hex(namespace_id),
        shard.shared.config.inner.server_name_suffix
    );
    let Some(tls) = tls_accept(
        &shard,
        stream,
        remote,
        Listener::Inner,
        shard.shared.tls.inner.clone(),
        &name,
    )
    .await
    else {
        return;
    };
    let certificates = peer_certificates(&tls);
    let principal = match certificates.first().map(crate::peer::principal_certificate) {
        Some(Ok(principal)) => principal,
        Some(Err(error)) => {
            shard
                .shared
                .penalty_box
                .failed(remote.ip(), Failure::Credential);
            tracing::warn!(%remote, error = format!("{error:#}"), "refused an inner client certificate");
            return;
        }
        None => {
            tracing::warn!(%remote, "an inner client presented no certificate");
            return;
        }
    };
    match shard
        .session(namespace_id)
        .filter(|session| !session.is_closing())
    {
        Some(session) => crate::inner::serve_client(shard, session, tls, principal, remote).await,
        None => {
            if crate::inner::refuse_principal(&shard, &principal, namespace_id, remote) {
                return;
            }
            crate::inner::serve_not_connected(
                shard,
                tls,
                namespace_id,
                principal.principal,
                remote,
            )
            .await;
        }
    }
}

async fn serve_nowhere(
    shard: Rc<Shard>,
    stream: TcpStream,
    namespace_id: u64,
    remote: SocketAddr,
    kind: Listener,
) {
    let name = format!(
        "{}.{}",
        crate::directory::namespace_hex(namespace_id),
        shard.shared.config.inner.server_name_suffix
    );
    let Some(tls) = tls_accept(
        &shard,
        stream,
        remote,
        kind,
        shard.shared.tls.inner.clone(),
        &name,
    )
    .await
    else {
        return;
    };
    let certificates = peer_certificates(&tls);
    let principal = match certificates.first().map(crate::peer::principal_certificate) {
        Some(Ok(principal)) => {
            if crate::inner::refuse_principal(&shard, &principal, namespace_id, remote) {
                return;
            }
            principal.principal
        }
        Some(Err(error)) => {
            shard
                .shared
                .penalty_box
                .failed(remote.ip(), Failure::Credential);
            tracing::warn!(%remote, error = format!("{error:#}"), "refused an inner client certificate");
            return;
        }
        None => return,
    };
    crate::inner::serve_not_connected(shard, tls, namespace_id, principal, remote).await;
}

struct RelayGauge;

impl RelayGauge {
    fn new() -> RelayGauge {
        metrics::gauge!("nightfall_relayed_connections").increment(1.0);
        RelayGauge
    }
}

impl Drop for RelayGauge {
    fn drop(&mut self) {
        metrics::gauge!("nightfall_relayed_connections").decrement(1.0);
    }
}

async fn relay(
    shard: Rc<Shard>,
    mut stream: TcpStream,
    remote: SocketAddr,
    namespace_id: u64,
    instance: &str,
    relay_address: &str,
) {
    let Ok(_permit) = shard.shared.relays.clone().try_acquire_owned() else {
        metrics::counter!("nightfall_rate_limited_total", "limit" => "max_relayed_connections")
            .increment(1);
        tracing::warn!(%remote, instance, "refused a relay: max_relayed_connections are open");
        return;
    };
    let local = match stream.local_addr() {
        Ok(local) => local,
        Err(error) => {
            tracing::debug!(%remote, %error, "a relayed connection has no local address");
            return;
        }
    };
    let connected =
        tokio::time::timeout(RELAY_CONNECT_TIMEOUT, TcpStream::connect(relay_address)).await;
    let mut upstream = match connected {
        Ok(Ok(upstream)) => upstream,
        Ok(Err(error)) => {
            metrics::counter!("nightfall_relay_failures_total").increment(1);
            tracing::warn!(%remote, instance, relay_address, %error, namespace_id = %crate::directory::namespace_hex(namespace_id), "the instance holding a node could not be reached");
            return;
        }
        Err(_) => {
            metrics::counter!("nightfall_relay_failures_total").increment(1);
            tracing::warn!(%remote, instance, relay_address, "connecting to the instance holding a node timed out");
            return;
        }
    };
    if let Err(error) = upstream.set_nodelay(true) {
        tracing::debug!(%error, "TCP_NODELAY could not be set on a relay");
    }
    if let Err(error) = keep_alive(&upstream) {
        tracing::warn!(%error, "TCP keepalive could not be set on a relay");
    }
    if let Err(error) = upstream
        .write_all(&crate::proxy::encode(remote, local))
        .await
    {
        tracing::warn!(%remote, instance, %error, "the PROXY header could not be written to a relay");
        return;
    }
    let _gauge = RelayGauge::new();
    tracing::debug!(%remote, instance, relay_address, namespace_id = %crate::directory::namespace_hex(namespace_id), "relaying an inner connection");
    match tokio::io::copy_bidirectional(&mut stream, &mut upstream).await {
        Ok((sent, received)) => {
            tracing::debug!(%remote, instance, sent, received, "a relay closed")
        }
        Err(error) => tracing::debug!(%remote, instance, %error, "a relay failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_keepalive_and_a_user_timeout_on_a_socket() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (connected, accepted) =
                tokio::join!(TcpStream::connect(address), listener.accept());
            let _connected = connected.unwrap();
            let (stream, _) = accepted.unwrap();
            keep_alive(&stream).unwrap();
            let socket = socket2::SockRef::from(&stream);
            assert!(socket.keepalive().unwrap());
            assert_eq!(socket.tcp_keepalive_time().unwrap(), KEEPALIVE_IDLE);
            assert_eq!(socket.tcp_keepalive_interval().unwrap(), KEEPALIVE_INTERVAL);
            assert_eq!(socket.tcp_keepalive_retries().unwrap(), KEEPALIVE_PROBES);
            assert_eq!(socket.tcp_user_timeout().unwrap(), Some(USER_TIMEOUT));
        });
    }

    #[test]
    fn reads_the_namespace_from_an_inner_server_name() {
        let suffix = "fleet.dusk.example";
        assert_eq!(
            namespace_of("00000000000000ab.fleet.dusk.example", suffix),
            Some(0xab)
        );
        assert_eq!(namespace_of("ab.fleet.dusk.example", suffix), None);
        assert_eq!(
            namespace_of("00000000000000AB.fleet.dusk.example", suffix),
            None
        );
        assert_eq!(namespace_of("00000000000000ab.other.example", suffix), None);
        assert_eq!(
            namespace_of("x.00000000000000ab.fleet.dusk.example", suffix),
            None
        );
        assert_eq!(namespace_of("fleet.dusk.example", suffix), None);
        assert_eq!(
            namespace_of("00000000000000abfleet.dusk.example", suffix),
            None
        );
    }
}
