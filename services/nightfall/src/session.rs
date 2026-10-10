use crate::audit::{SessionFacts, detail, session_event, setup_call};
use crate::backoff::jittered;
use crate::directory::{LocalEntry, NodeIdentity, namespace_hex, unix_microseconds};
use crate::events::{DisconnectReason, SessionRecord, format_unix_seconds, now};
use crate::kafka::unix_milliseconds;
use crate::peer::NodeCertificate;
use crate::server::Shared;
use crate::shard::{Control, Shard};
use capnp::capability::Promise;
use capnp::traits::HasTypeId;
use capnp_rpc::rpc_twoparty_capnp::Side;
use dusk_capnp::dusk_capnp::dusk;
use nightfall_ledger::entry::Event;
use nightfall_membrane::admission::IntendedProcesses;
use nightfall_membrane::membrane::Membrane;
use nightfall_membrane::node::{NodeLink, SessionIdentity};
use nightfall_membrane::permissions::Permissions;
use nightfall_membrane::schema::{Bundle, ProgramVersion};
use nightfall_provisioning::state::Lifecycle;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(10);
pub const HEARTBEAT_JITTER: f64 = 0.2;
pub const HEARTBEAT_MISSES: u32 = 2;
pub const SESSION_CLOSED_REASON: &str = "node_session_closed";
pub const LIFECYCLE_CHANGED_REASON: &str = "lifecycle_changed";
pub const CERTIFICATE_DENIED_REASON: &str = "certificate_denied";
pub const KILLED_REASON: &str = "killed";

const NAMESPACE_ID_METHOD: u16 = 9;
const PROGRAMS_METHOD: u16 = 8;
const DUSK_METHOD: u16 = 10;

pub struct InnerClient {
    pub membrane: Rc<Membrane>,
    pub principal: String,
    pub fingerprint: [u8; 32],
    pub remote: SocketAddr,
    pub connected_at: String,
}

pub struct NodeSession {
    pub namespace_id: u64,
    pub epoch: u64,
    pub identity: NodeIdentity,
    pub device_id: String,
    pub installation_id: String,
    pub record: SessionRecord,
    pub bundle: Arc<Bundle>,
    pub dusk: dusk::Client,
    pub quarantined: Cell<bool>,
    pub last_seen_ms: Arc<AtomicU64>,
    closing: CancellationToken,
    reason: Cell<Option<DisconnectReason>>,
    killed_by: RefCell<Option<String>>,
    clients: RefCell<HashMap<String, Rc<InnerClient>>>,
    shard: Weak<Shard>,
}

fn dusk_interface() -> u64 {
    <dusk::Client as HasTypeId>::TYPE_ID
}

impl NodeSession {
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn installation_id(&self) -> &str {
        &self.installation_id
    }

    pub fn facts(&self) -> SessionFacts<'_> {
        SessionFacts {
            device_id: self.device_id(),
            installation_id: self.installation_id(),
            namespace_id: Some(self.namespace_id),
            epoch: Some(self.epoch),
        }
    }

    pub fn is_closing(&self) -> bool {
        self.closing.is_cancelled()
    }

    pub fn close(&self, reason: DisconnectReason) {
        if self.reason.get().is_none() {
            self.reason.set(Some(reason));
        }
        self.closing.cancel();
    }

    pub fn kill(&self, by: &str) {
        if self.reason.get().is_none() {
            *self.killed_by.borrow_mut() = Some(by.to_string());
        }
        self.close(DisconnectReason::Killed);
    }

    pub fn identity_snapshot(&self) -> SessionIdentity {
        let shard = self.shard.upgrade();
        SessionIdentity {
            device_id: self.device_id().to_string(),
            installation_id: self.installation_id().to_string(),
            namespace_id: self.namespace_id,
            instance: shard
                .map(|shard| shard.shared.config.instance.clone())
                .unwrap_or_default(),
            quarantined: self.quarantined.get(),
        }
    }

    pub fn add_client(&self, client: Rc<InnerClient>) {
        self.clients
            .borrow_mut()
            .insert(client.membrane.session_id().to_string(), client);
    }

    pub fn remove_client(&self, session_id: &str) {
        self.clients.borrow_mut().remove(session_id);
    }

    fn clients(&self) -> Vec<Rc<InnerClient>> {
        self.clients.borrow().values().cloned().collect()
    }

    pub fn drop_clients(&self, reason: &str) {
        let clients: Vec<Rc<InnerClient>> = self
            .clients
            .borrow_mut()
            .drain()
            .map(|(_, client)| client)
            .collect();
        for client in clients {
            client.membrane.drop_membrane(reason);
        }
    }

    pub fn node_state_changed(&self) {
        let Some(shard) = self.shard.upgrade() else {
            return;
        };
        let state = shard.shared.node_states.effective(self.identity);
        if state.is_some_and(Lifecycle::blocks_certificates) {
            tracing::info!(
                device_id = %self.device_id(),
                installation_id = %self.installation_id(),
                namespace_id = %namespace_hex(self.namespace_id),
                epoch = self.epoch,
                lifecycle = state.map(Lifecycle::name),
                "closing a node session whose node was revoked or retired"
            );
            self.close(DisconnectReason::Revoked);
            return;
        }
        let quarantined = state == Some(Lifecycle::Quarantined);
        if quarantined == self.quarantined.get() {
            return;
        }
        self.quarantined.set(quarantined);
        if let Some(entry) = shard.shared.directory().local_mut(self.namespace_id)
            && entry.epoch == self.epoch
        {
            entry.quarantined = quarantined;
        }
        tracing::info!(
            device_id = %self.device_id(),
            installation_id = %self.installation_id(),
            namespace_id = %namespace_hex(self.namespace_id),
            epoch = self.epoch,
            quarantined,
            clients = self.clients.borrow().len(),
            "the node's quarantine changed; dropping its client membranes"
        );
        self.drop_clients(LIFECYCLE_CHANGED_REASON);
    }

    pub fn permissions_changed(&self, permissions: &Permissions) {
        for client in self.clients() {
            if permissions.denies_certificate(&client.fingerprint) {
                client.membrane.drop_membrane(CERTIFICATE_DENIED_REASON);
            } else {
                client.membrane.reapply(permissions);
            }
            if client.membrane.dropped() {
                self.remove_client(client.membrane.session_id());
            }
        }
    }

    pub fn kill_client(&self, session_id: &str, by: &str) -> bool {
        let client = self.clients.borrow_mut().remove(session_id);
        match client {
            Some(client) => {
                client.membrane.drop_membrane_by(KILLED_REASON, by);
                true
            }
            None => false,
        }
    }

    pub fn describe(&self) -> Value {
        let clients: Vec<Value> = self
            .clients()
            .iter()
            .map(|client| {
                json!({
                    "session_id": client.membrane.session_id(),
                    "principal": client.principal,
                    "remote_address": client.remote.to_string(),
                    "connected_at": client.connected_at,
                })
            })
            .collect();
        json!({
            "namespace_id": namespace_hex(self.namespace_id),
            "device_id": self.device_id(),
            "installation_id": self.installation_id(),
            "epoch": self.epoch,
            "tenant": self.record.tenant,
            "remote_address": self.record.remote_address,
            "cert_fingerprint": self.record.cert_fingerprint,
            "cert_not_after": self.record.cert_not_after,
            "connected_at": self.record.connected_at,
            "last_seen": crate::events::format_unix_milliseconds(self.last_seen_ms.load(Ordering::Relaxed) as i64),
            "quarantined": self.quarantined.get(),
            "bundle": self.bundle.name(),
            "clients": clients,
        })
    }

    pub fn provenance(&self) -> Value {
        let clients: Vec<Value> = self
            .clients()
            .iter()
            .map(|client| {
                let capabilities: Vec<Value> = client
                    .membrane
                    .provenance()
                    .into_iter()
                    .map(|entry| {
                        json!({
                            "cap_id": entry.cap_id,
                            "parent_cap_id": entry.parent_cap_id,
                            "call_id": entry.call_id,
                            "action": entry.action,
                            "interface_id": format!("{:016x}", entry.interface_id),
                            "principal": entry.principal,
                            "pid": entry.pid.to_string(),
                            "live": entry.live,
                        })
                    })
                    .collect();
                json!({
                    "session_id": client.membrane.session_id(),
                    "principal": client.principal,
                    "capabilities": capabilities,
                })
            })
            .collect();
        json!({
            "namespace_id": namespace_hex(self.namespace_id),
            "epoch": self.epoch,
            "clients": clients,
        })
    }
}

pub struct SessionLink {
    session: Weak<NodeSession>,
    identity: SessionIdentity,
    epoch: u64,
    intended_processes: Arc<IntendedProcesses>,
}

impl SessionLink {
    pub fn new(
        session: &Rc<NodeSession>,
        intended_processes: Arc<IntendedProcesses>,
    ) -> Rc<SessionLink> {
        Rc::new(SessionLink {
            session: Rc::downgrade(session),
            identity: session.identity_snapshot(),
            epoch: session.epoch,
            intended_processes,
        })
    }
}

impl NodeLink for SessionLink {
    fn epoch(&self) -> u64 {
        self.epoch
    }

    fn identity(&self) -> &SessionIdentity {
        &self.identity
    }

    fn fresh_dusk(&self) -> Promise<dusk::Client, capnp::Error> {
        let Some(session) = self.session.upgrade() else {
            return Promise::err(capnp::Error::disconnected(format!(
                "node {} is not connected",
                namespace_hex(self.identity.namespace_id)
            )));
        };
        if let Some(shard) = session.shard.upgrade() {
            shard.audit.record_event(setup_call(
                &session.facts(),
                "Dusk.dusk",
                dusk_interface(),
                DUSK_METHOD,
            ));
        }
        let request = session.dusk.dusk_request();
        Promise::from_future(async move { request.send().promise.await?.get()?.get_result() })
    }

    fn closed(&self) -> bool {
        self.session
            .upgrade()
            .is_none_or(|session| session.is_closing())
    }

    fn intended_processes(&self) -> Option<&IntendedProcesses> {
        Some(&self.intended_processes)
    }
}

enum SetupFailure {
    Rpc(capnp::Error),
    BindingConflict {
        namespace_id: u64,
        bound: NodeIdentity,
        instance: Arc<str>,
    },
}

impl From<capnp::Error> for SetupFailure {
    fn from(error: capnp::Error) -> SetupFailure {
        SetupFailure::Rpc(error)
    }
}

struct Setup {
    namespace_id: u64,
    bundle: Arc<Bundle>,
}

fn conflict(shard: &Shard, namespace_id: u64, identity: NodeIdentity) -> Result<(), SetupFailure> {
    let directory = shard.shared.directory();
    match directory.binding(namespace_id) {
        Some((bound, instance)) if bound != identity => Err(SetupFailure::BindingConflict {
            namespace_id,
            bound,
            instance,
        }),
        _ => Ok(()),
    }
}

async fn setup(
    shard: &Shard,
    dusk: &dusk::Client,
    node: &NodeCertificate,
    identity: NodeIdentity,
) -> Result<Setup, SetupFailure> {
    let unbound = SessionFacts {
        device_id: &node.device_id,
        installation_id: &node.installation_id,
        namespace_id: None,
        epoch: None,
    };
    shard.audit.record_event(setup_call(
        &unbound,
        "Dusk.namespaceId",
        dusk_interface(),
        NAMESPACE_ID_METHOD,
    ));
    let namespace_id = dusk
        .namespace_id_request()
        .send()
        .promise
        .await?
        .get()?
        .get_result();
    conflict(shard, namespace_id, identity)?;
    let bound = SessionFacts {
        namespace_id: Some(namespace_id),
        ..unbound
    };
    shard.audit.record_event(setup_call(
        &bound,
        "Dusk.programs",
        dusk_interface(),
        PROGRAMS_METHOD,
    ));
    let response = dusk.programs_request().send().promise.await?;
    let entries = response.get()?.get_program_entries()?;
    let mut programs = Vec::with_capacity(entries.len() as usize);
    for entry in entries.iter() {
        programs.push(ProgramVersion {
            program_id: entry.get_program_id(),
            version: entry
                .get_version()?
                .to_string()
                .map_err(capnp::Error::from)?,
            git_revision: entry
                .get_git_revision()?
                .to_string()
                .map_err(capnp::Error::from)?,
        });
    }
    let bundle = shard.shared.registry.bundle_for(&programs);
    Ok(Setup {
        namespace_id,
        bundle,
    })
}

fn refuse(shard: &Shard, node: &NodeCertificate, namespace_id: Option<u64>, reason: &str) {
    metrics::counter!("nightfall_sessions_refused_total", "reason" => reason.to_string())
        .increment(1);
    shard.audit.record_event(session_event(
        Event::SessionRefused,
        &SessionFacts {
            device_id: &node.device_id,
            installation_id: &node.installation_id,
            namespace_id,
            epoch: None,
        },
        Some(detail(&[("reason", Value::String(reason.to_string()))])),
    ));
}

fn binding_conflict(
    shard: &Shard,
    node: &NodeCertificate,
    remote: SocketAddr,
    namespace_id: u64,
    bound: NodeIdentity,
    instance: &str,
) {
    metrics::counter!("nightfall_binding_conflicts_total").increment(1);
    tracing::error!(
        device_id = %node.device_id,
        installation_id = %node.installation_id,
        namespace_id = %namespace_hex(namespace_id),
        bound_device_id = %bound.device_id(),
        bound_installation_id = %bound.installation_id(),
        bound_instance = %instance,
        %remote,
        "refused a node session: its namespace id is bound to another identity"
    );
    shard.audit.record_event(session_event(
        Event::BindingConflict,
        &SessionFacts {
            device_id: &node.device_id,
            installation_id: &node.installation_id,
            namespace_id: Some(namespace_id),
            epoch: None,
        },
        Some(detail(&[
            ("bound_device_id", Value::String(bound.device_id())),
            (
                "bound_installation_id",
                Value::String(bound.installation_id()),
            ),
            ("bound_instance", Value::String(instance.to_string())),
        ])),
    ));
}

pub fn admit(shard: &Shard, node: &NodeCertificate, identity: NodeIdentity) -> Option<bool> {
    let state = shard.shared.node_states.effective(identity);
    if let Some(lifecycle) = state.filter(|lifecycle| lifecycle.blocks_certificates()) {
        tracing::info!(
            device_id = %node.device_id,
            installation_id = %node.installation_id,
            lifecycle = lifecycle.name(),
            "refused a node whose lifecycle forbids a session"
        );
        refuse(shard, node, None, lifecycle.name());
        return None;
    }
    if !shard
        .shared
        .setups
        .admit(&node.device_id, &node.installation_id)
    {
        metrics::counter!("nightfall_rate_limited_total", "limit" => "session_setups_per_identity_per_5s")
            .increment(1);
        tracing::debug!(
            device_id = %node.device_id,
            installation_id = %node.installation_id,
            "refused a session setup above the per-identity rate"
        );
        return None;
    }
    Some(state == Some(Lifecycle::Quarantined))
}

fn rejection_observer(shard: &Rc<Shard>, node: &NodeCertificate) -> impl Fn(&str) + 'static {
    let audit = shard.audit.clone();
    let device_id = node.device_id.clone();
    let installation_id = node.installation_id.clone();
    move |kind: &str| {
        tracing::error!(
            device_id = %device_id,
            installation_id = %installation_id,
            kind,
            "a node sent a rejected rpc message; the link is closed"
        );
        audit.record_event(session_event(
            Event::RpcRejected,
            &SessionFacts {
                device_id: &device_id,
                installation_id: &installation_id,
                namespace_id: None,
                epoch: None,
            },
            Some(detail(&[("kind", Value::String(kind.to_string()))])),
        ));
    }
}

async fn heartbeat(
    dusk: dusk::Client,
    period: Duration,
    last_seen: Arc<AtomicU64>,
    namespace_id: u64,
) {
    let mut misses = 0u32;
    loop {
        tokio::time::sleep(jittered(period, HEARTBEAT_JITTER)).await;
        let answered = tokio::time::timeout(
            HEARTBEAT_TIMEOUT.min(period),
            dusk.time_request().send().promise,
        )
        .await;
        match answered {
            Ok(Ok(_)) => {
                misses = 0;
                last_seen.store(unix_milliseconds() as u64, Ordering::Relaxed);
            }
            Ok(Err(error)) => {
                misses += 1;
                metrics::counter!("nightfall_heartbeat_misses_total").increment(1);
                tracing::debug!(namespace_id = %namespace_hex(namespace_id), misses, %error, "a heartbeat failed");
            }
            Err(_) => {
                misses += 1;
                metrics::counter!("nightfall_heartbeat_misses_total").increment(1);
                tracing::debug!(namespace_id = %namespace_hex(namespace_id), misses, "a heartbeat timed out");
            }
        }
        if misses >= HEARTBEAT_MISSES {
            return;
        }
    }
}

pub struct Counted {
    shared: Arc<Shared>,
}

impl Counted {
    pub fn take(shared: &Arc<Shared>) -> Option<Counted> {
        let previous = shared.sessions.fetch_add(1, Ordering::AcqRel);
        let counted = Counted {
            shared: shared.clone(),
        };
        (previous < shared.max_sessions).then_some(counted)
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.shared.sessions.fetch_sub(1, Ordering::AcqRel);
    }
}

pub async fn serve_node<Stream>(
    shard: Rc<Shard>,
    stream: Stream,
    remote: SocketAddr,
    node: NodeCertificate,
    counted: Counted,
) where
    Stream: AsyncRead + AsyncWrite + Unpin + 'static,
{
    let identity = match NodeIdentity::parse(&node.device_id, &node.installation_id) {
        Some(identity) => identity,
        None => {
            metrics::counter!("nightfall_sessions_refused_total", "reason" => "invalid_certificate")
                .increment(1);
            tracing::warn!(device_id = %node.device_id, installation_id = %node.installation_id, "a node certificate names invalid ids");
            return;
        }
    };
    let Some(quarantined) = admit(&shard, &node, identity) else {
        return;
    };
    let config = &shard.shared.config;
    let (open_slot, close_slot) = match shard.audit.reserve_session() {
        Ok(slots) => slots,
        Err(refused) => {
            tracing::warn!(device_id = %node.device_id, installation_id = %node.installation_id, %refused, "refused a node session: the ledger has no room");
            return;
        }
    };
    let _counted = counted;

    let mut system = crate::rpc::system(
        stream,
        Side::Client,
        shard.shared.limits.max_message_bytes,
        None,
        rejection_observer(&shard, &node),
    );
    let dusk: dusk::Client = system.bootstrap(Side::Server);
    let disconnector = system.get_disconnector();
    let (finished_sender, finished) = tokio::sync::oneshot::channel();
    let link = tokio::task::spawn_local(async move {
        let outcome = system.await;
        if finished_sender.send(outcome).is_err() {
            tracing::debug!("the node session ended before its link");
        }
    });

    let setup_timeout = Duration::from_millis(config.fleet.session_setup_timeout_ms);
    let established =
        tokio::time::timeout(setup_timeout, setup(&shard, &dusk, &node, identity)).await;
    let established = match established {
        Ok(Ok(established)) => established,
        Ok(Err(SetupFailure::Rpc(error))) => {
            tracing::warn!(device_id = %node.device_id, installation_id = %node.installation_id, %remote, %error, "a node session failed during setup");
            link.abort();
            return;
        }
        Ok(Err(SetupFailure::BindingConflict {
            namespace_id,
            bound,
            instance,
        })) => {
            binding_conflict(&shard, &node, remote, namespace_id, bound, &instance);
            link.abort();
            return;
        }
        Err(_) => {
            tracing::warn!(device_id = %node.device_id, installation_id = %node.installation_id, %remote, timeout_ms = config.fleet.session_setup_timeout_ms, "a node session did not finish its setup in time");
            refuse(&shard, &node, None, "setup_timeout");
            link.abort();
            return;
        }
    };
    let namespace_id = established.namespace_id;

    let connected_at = now();
    let last_seen_ms = Arc::new(AtomicU64::new(unix_milliseconds() as u64));
    let (epoch, previous) = {
        let mut directory = shard.shared.directory();
        if let Some((bound, instance)) = directory.binding(namespace_id)
            && bound != identity
        {
            drop(directory);
            binding_conflict(&shard, &node, remote, namespace_id, bound, &instance);
            link.abort();
            return;
        }
        let issued = directory.issue_epoch(namespace_id, identity, unix_microseconds());
        let previous = directory.register_local(
            namespace_id,
            LocalEntry {
                shard: shard.index,
                identity,
                epoch: issued.epoch,
                connected_at: connected_at.clone(),
                last_seen_ms: last_seen_ms.clone(),
                tenant: node.tenant.clone(),
                remote_address: remote,
                cert_fingerprint: node.fingerprint.clone(),
                cert_not_after: format_unix_seconds(node.not_after_unix),
                quarantined,
                epoch_without_history: issued.without_history,
            },
        );
        (issued.epoch, previous)
    };
    if let Some(previous) = previous.filter(|previous| previous.shard != shard.index) {
        shard.shared.send(
            previous.shard,
            Control::Close {
                namespace_id,
                epoch: previous.epoch,
                reason: DisconnectReason::Replaced,
            },
        );
    }

    let session = Rc::new(NodeSession {
        namespace_id,
        epoch,
        identity,
        device_id: node.device_id.clone(),
        installation_id: node.installation_id.clone(),
        record: SessionRecord {
            identity,
            namespace_id,
            epoch,
            instance: config.instance.clone(),
            inner_address: config.inner.advertise.clone(),
            remote_address: nightfall_provisioning::events::remote_address(remote),
            tenant: node.tenant.clone(),
            cert_fingerprint: node.fingerprint.clone(),
            cert_not_after: format_unix_seconds(node.not_after_unix),
            connected_at,
        },
        bundle: established.bundle,
        dusk: dusk.clone(),
        quarantined: Cell::new(quarantined),
        last_seen_ms: last_seen_ms.clone(),
        closing: CancellationToken::new(),
        reason: Cell::new(None),
        killed_by: RefCell::new(None),
        clients: RefCell::new(HashMap::new()),
        shard: Rc::downgrade(&shard),
    });
    let displaced = shard.add_session(session.clone());
    if let Some(displaced) = displaced {
        displaced.close(DisconnectReason::Replaced);
    }
    shard.update_gauge();
    shard.shared.events.publish(
        &config.kafka.topics.connections,
        Some(session.record.key()),
        &session.record.message(None),
    );
    let mut open_slot = open_slot;
    if let Err(refused) =
        open_slot.record(session_event(Event::SessionOpen, &session.facts(), None))
    {
        tracing::error!(%refused, "the ledger refused a session_open entry; it is not recorded");
    }
    tracing::info!(
        device_id = %session.device_id(),
        installation_id = %session.installation_id(),
        namespace_id = %namespace_hex(namespace_id),
        epoch,
        tenant = ?node.tenant,
        %remote,
        quarantined,
        bundle = %session.bundle.name(),
        shard = shard.index,
        "node session open"
    );
    session.node_state_changed();

    let expires_in = Duration::from_secs(
        node.not_after_unix
            .saturating_sub(unix_milliseconds() / 1000)
            .max(0) as u64,
    );
    let heartbeat_period = Duration::from_secs(config.fleet.heartbeat_seconds);
    let reason = tokio::select! {
        () = session.closing.cancelled() => session.reason.get().unwrap_or(DisconnectReason::Shutdown),
        outcome = finished => match outcome {
            Ok(Ok(())) => DisconnectReason::NodeClosed,
            Ok(Err(error)) => {
                tracing::info!(namespace_id = %namespace_hex(namespace_id), epoch, %error, "a node link failed");
                DisconnectReason::TransportError
            }
            Err(_) => DisconnectReason::TransportError,
        },
        () = heartbeat(dusk, heartbeat_period, last_seen_ms, namespace_id) => DisconnectReason::IdleTimeout,
        () = tokio::time::sleep(expires_in) => DisconnectReason::CertExpired,
    };
    session.close(reason);
    let reason = session.reason.get().unwrap_or(reason);

    shard.remove_session(&session);
    shard.update_gauge();
    shard
        .shared
        .directory()
        .unregister_local(namespace_id, epoch);
    session.drop_clients(SESSION_CLOSED_REASON);
    shard.shared.events.publish(
        &config.kafka.topics.connections,
        Some(session.record.key()),
        &session.record.message(Some(reason)),
    );
    let mut close_detail = vec![("reason", Value::String(reason.as_str().to_string()))];
    let killed_by = session.killed_by.borrow().clone();
    if let Some(by) = &killed_by {
        close_detail.push(("by", Value::String(by.clone())));
    }
    let mut close_slot = close_slot;
    if let Err(refused) = close_slot.record(session_event(
        Event::SessionClose,
        &session.facts(),
        Some(detail(&close_detail)),
    )) {
        tracing::error!(%refused, "the ledger refused a session_close entry; it is not recorded");
    }
    if tokio::time::timeout(crate::inner::DISCONNECT_GRACE, disconnector)
        .await
        .is_err()
    {
        tracing::debug!(namespace_id = %namespace_hex(namespace_id), "a closed node link did not disconnect in time");
    }
    link.abort();
    tracing::info!(
        device_id = %session.device_id(),
        installation_id = %session.installation_id(),
        namespace_id = %namespace_hex(namespace_id),
        epoch,
        reason = reason.as_str(),
        killed_by,
        "node session closed"
    );
}
