use crate::audit::LedgerAudit;
use crate::census::{CensusProducer, CensusSettings};
use crate::config::Config;
use crate::directory::Directory;
use crate::events::{EnrollmentPublisher, EventPublisher, now};
use crate::kafka::Broker;
use crate::limits::{AddressSlots, HandshakeBucket, PenaltyBox, SetupRate};
use crate::node_state::NodeStates;
use crate::shard::{Control, ShardHandle, ShardListeners};
use crate::tls::{Reloadable, ReloadingCertificate, ReloadingClientVerifier};
use anyhow::Context;
use arc_swap::ArcSwap;
use nightfall_ledger::log::{LedgerLog, LogErrorKind};
use nightfall_ledger::signing::CheckpointSigner;
use nightfall_ledger::writer::{
    LedgerConfig, LedgerObserver, LedgerStatus, LedgerThread, LedgerWriter,
};
use nightfall_membrane::admission::IntendedProcesses;
use nightfall_membrane::limits::{InstanceLimits, Limits};
use nightfall_membrane::permissions::Permissions;
use nightfall_membrane::schema::SchemaRegistry;
use nightfall_provisioning::config::{ProvisioningConfig, ProvisioningFiles, load_certificates};
use nightfall_provisioning::renew::{FleetClientTrust, RenewClientVerifier};
use nightfall_provisioning::server::Provisioning;
use nightfall_provisioning::step_ca::{StepCaClient, StepCaConfig};
use rustls::ServerConfig;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tokio_util::sync::CancellationToken;

pub const RELOAD_PERIOD: Duration = Duration::from_secs(30);
pub const GAUGE_PERIOD: Duration = Duration::from_secs(5);
pub const LISTEN_BACKLOG: u32 = 4096;
pub const DRAIN_CENSUS_TIMEOUT: Duration = Duration::from_secs(10);
pub const CENSUS_START_POLL: Duration = Duration::from_millis(100);

pub struct TlsConfigs {
    pub fleet: Arc<ServerConfig>,
    pub provision: Arc<ServerConfig>,
    pub inner: Arc<ServerConfig>,
    pub admin: Option<Arc<ServerConfig>>,
}

#[derive(Default)]
pub struct Readiness {
    pub node_state: AtomicBool,
    pub directory: AtomicBool,
    pub topics: AtomicBool,
    pub draining: AtomicBool,
}

pub struct Shared {
    pub config: Config,
    pub limits: Limits,
    pub max_sessions: u64,
    pub instance_limits: Arc<InstanceLimits>,
    pub directory: Arc<Mutex<Directory>>,
    pub node_states: Arc<NodeStates>,
    pub intended_processes: Arc<IntendedProcesses>,
    pub permissions: ArcSwap<Permissions>,
    pub registry: SchemaRegistry,
    pub audit: LedgerAudit,
    pub param_key: Arc<[u8]>,
    pub events: EventPublisher,
    pub handshakes: HandshakeBucket,
    pub penalty_box: Arc<PenaltyBox>,
    pub provisioning_connections: Arc<tokio::sync::Semaphore>,
    pub provisioning_addresses: Arc<AddressSlots>,
    pub setups: SetupRate,
    pub tls: TlsConfigs,
    pub provisioning: Arc<Provisioning>,
    pub shards: Vec<ShardHandle>,
    pub relays: Arc<tokio::sync::Semaphore>,
    pub readiness: Readiness,
    pub accepting: CancellationToken,
    pub accepting_inner: CancellationToken,
    pub sessions: AtomicU64,
    pub started_at: String,
}

impl Shared {
    pub fn ready(&self) -> Result<(), &'static str> {
        if self.readiness.draining.load(Ordering::Acquire) {
            return Err("draining");
        }
        if !self.readiness.node_state.load(Ordering::Acquire) {
            return Err("node state not caught up");
        }
        if !self.readiness.directory.load(Ordering::Acquire) {
            return Err("directory not caught up");
        }
        if !self.intended_processes.caught_up() {
            return Err("intended processes not caught up");
        }
        if !self.readiness.topics.load(Ordering::Acquire) {
            return Err("a Kafka topic is missing or misconfigured");
        }
        if self.audit.writer().status() != LedgerStatus::Ready {
            return Err("ledger not ready");
        }
        Ok(())
    }

    pub fn send(&self, shard: usize, control: Control) {
        match self.shards.get(shard) {
            Some(handle) => {
                if handle.control.send(control).is_err() {
                    tracing::warn!(shard, "a shard has stopped; its command is dropped");
                }
            }
            None => tracing::error!(shard, "a command names a shard that does not exist"),
        }
    }

    pub fn broadcast(&self, control: impl Fn() -> Control) {
        for shard in 0..self.shards.len() {
            self.send(shard, control());
        }
    }

    pub fn directory(&self) -> std::sync::MutexGuard<'_, Directory> {
        self.directory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn flip(&self, scope: crate::node_state::Scope) {
        for (shard, handle) in self.shards.iter().enumerate() {
            let first = {
                let mut flips = handle
                    .flips
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let first = flips.is_empty();
                flips.insert(scope);
                first
            };
            if first {
                self.send(shard, Control::NodeState);
            }
        }
    }

    pub fn inner_address(&self) -> &str {
        &self.config.inner.advertise
    }
}

pub struct Services {
    pub broker: Arc<dyn Broker>,
    pub ledger_log: Box<dyn LedgerLog>,
}

struct MetricsObserver;

impl LedgerObserver for MetricsObserver {
    fn committed(&self, _entries: usize, duration: Duration) {
        metrics::histogram!("nightfall_ledger_commit_seconds").record(duration.as_secs_f64());
    }

    fn delivery_failed(&self, kind: LogErrorKind) {
        metrics::counter!("nightfall_ledger_delivery_failures_total").increment(1);
        tracing::warn!(?kind, "a ledger transaction was not delivered");
    }

    fn status_changed(&self, status: LedgerStatus) {
        tracing::warn!(?status, "the ledger changed status");
    }
}

fn read(path: &Path, what: &str) -> anyhow::Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("read the {what} {}", path.display()))
}

pub fn load_permissions(path: &Path) -> anyhow::Result<Permissions> {
    Permissions::from_toml(&read(path, "permissions")?)
        .with_context(|| format!("load the permissions {}", path.display()))
}

fn expected_cleanup(config: &Config) -> Vec<(String, &'static str)> {
    let topics = &config.kafka.topics;
    vec![
        (topics.connections.clone(), "delete"),
        (topics.census.clone(), "compact"),
        (topics.ledger.clone(), "delete"),
        (topics.enrollments.clone(), "delete"),
        (topics.node_state.clone(), "compact"),
        (topics.intended_processes.clone(), "compact"),
    ]
}

pub fn check_topics(broker: &dyn Broker, config: &Config) -> Vec<String> {
    let mut problems = Vec::new();
    for (topic, cleanup) in expected_cleanup(config) {
        match broker.describe(&topic) {
            Ok(Some(description)) => {
                let policies: std::collections::BTreeSet<&str> = description
                    .cleanup_policy
                    .split(',')
                    .map(str::trim)
                    .collect();
                if policies != std::collections::BTreeSet::from([cleanup]) {
                    problems.push(format!(
                        "{topic} has cleanup.policy {} instead of {cleanup}",
                        description.cleanup_policy
                    ));
                }
            }
            Ok(None) => problems.push(format!("{topic} does not exist")),
            Err(error) => problems.push(format!("{topic} could not be described: {error:#}")),
        }
    }
    problems
}

fn bind(name: &str, address: &str) -> anyhow::Result<std::net::TcpListener> {
    let address: SocketAddr = address
        .parse()
        .with_context(|| format!("{name} {address:?} is not an address"))?;
    let socket = if address.is_ipv4() {
        tokio::net::TcpSocket::new_v4()
    } else {
        tokio::net::TcpSocket::new_v6()
    }
    .with_context(|| format!("create the {name} socket"))?;
    socket
        .set_reuseaddr(true)
        .with_context(|| format!("set SO_REUSEADDR on the {name} socket"))?;
    socket
        .bind(address)
        .with_context(|| format!("bind {name} to {address}"))?;
    let listener = socket
        .listen(LISTEN_BACKLOG)
        .with_context(|| format!("listen on {name} {address}"))?;
    let listener = listener
        .into_std()
        .with_context(|| format!("detach the {name} listener"))?;
    tracing::info!(listener = name, address = %listener.local_addr()?, "listening");
    Ok(listener)
}

#[derive(Clone, Copy, Debug)]
pub struct BoundAddresses {
    pub fleet: SocketAddr,
    pub provision: SocketAddr,
    pub inner: SocketAddr,
    pub relay: SocketAddr,
    pub admin: SocketAddr,
}

pub struct Instance {
    pub shared: Arc<Shared>,
    pub addresses: BoundAddresses,
    runtime: Option<tokio::runtime::Runtime>,
    shard_threads: Vec<std::thread::JoinHandle<()>>,
    ledger_thread: Option<LedgerThread>,
    consumers: CancellationToken,
    consumer_threads: Vec<std::thread::JoinHandle<()>>,
    census: Option<tokio::task::JoinHandle<CensusProducer>>,
    census_stop: CancellationToken,
    background: CancellationToken,
    census_producer: Arc<dyn crate::kafka::RecordProducer>,
}

struct AdminTls {
    server: Arc<ServerConfig>,
    reloadables: Vec<Arc<dyn Reloadable>>,
}

struct FleetClientRoots {
    verifier: Arc<ReloadingClientVerifier>,
    renewal: Arc<FleetClientTrust>,
}

impl Reloadable for FleetClientRoots {
    fn name(&self) -> &str {
        self.verifier.name()
    }

    fn reload_if_changed(&self) -> anyhow::Result<bool> {
        if !self.verifier.reload_if_changed()? {
            return Ok(false);
        }
        self.renewal
            .replace_roots(&self.verifier.certificates())
            .map_err(|error| anyhow::anyhow!("the renewal trust kept its roots: {error}"))?;
        Ok(true)
    }
}

fn admin_tls(config: &Config) -> anyhow::Result<Option<AdminTls>> {
    if !config.admin_tls() {
        return Ok(None);
    }
    let certificate =
        ReloadingCertificate::load("admin", &config.admin.certificate, &config.admin.key)?;
    let verifier = ReloadingClientVerifier::load("admin client", &config.admin.client_ca, false)?;
    Ok(Some(AdminTls {
        server: crate::tls::server_config(certificate.clone(), verifier.clone(), false)?,
        reloadables: vec![certificate, verifier],
    }))
}

pub fn start(config: Config, services: Services) -> anyhow::Result<Instance> {
    crate::admin::install_metrics();
    let limits = config.limits()?;
    let reserved = crate::rlimit::reserved_descriptors(&limits);
    let max_sessions = match crate::rlimit::raise_descriptor_limit() {
        Ok(limit) => crate::rlimit::clamp_sessions(config.fleet.max_sessions, reserved, limit),
        Err(error) => {
            tracing::warn!(%error, "the open file limit could not be read; fleet.max_sessions is not clamped");
            config.fleet.max_sessions
        }
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("nightfall-control")
        .enable_all()
        .build()
        .context("start the control runtime")?;

    let fleet_certificate =
        ReloadingCertificate::load("fleet", &config.fleet.certificate, &config.fleet.key)?;
    let provision_certificate = ReloadingCertificate::load(
        "provision",
        &config.provision.certificate,
        &config.provision.key,
    )?;
    let inner_certificate =
        ReloadingCertificate::load("inner", &config.inner.certificate, &config.inner.key)?;
    let fleet_clients =
        ReloadingClientVerifier::load("fleet client", &config.fleet.client_ca, true)?;
    let inner_clients =
        ReloadingClientVerifier::load("inner client", &config.inner.client_ca, true)?;
    ReloadingClientVerifier::keep_apart(&fleet_clients, &inner_clients)?;
    let admin = admin_tls(&config)?;
    if admin.is_none() {
        tracing::warn!(
            "the admin API is disabled: set admin.certificate, admin.key and admin.client_ca to enable it"
        );
    }

    let registry =
        SchemaRegistry::load_directory(&config.schemas.directory).with_context(|| {
            format!(
                "load the schema bundles in {}",
                config.schemas.directory.display()
            )
        })?;
    let permissions = load_permissions(&config.permissions.file)?;
    let param_key = nightfall_ledger::param_hash::decode_hex_key(
        read(&config.ledger.param_key_file, "ledger param key")?.trim(),
    )
    .map_err(|error| anyhow::anyhow!("{}: {error}", config.ledger.param_key_file.display()))?;
    let signer = CheckpointSigner::load(&config.ledger.signing_key_file).map_err(|error| {
        anyhow::anyhow!("{}: {error}", config.ledger.signing_key_file.display())
    })?;

    let topic_problems = check_topics(services.broker.as_ref(), &config);
    for problem in &topic_problems {
        tracing::error!(problem = %problem, "a Kafka topic is not ready");
    }

    let partition = config.ledger_partition()?;
    let mut ledger_config = LedgerConfig::new(&config.instance, partition);
    ledger_config.queue_entries = config.ledger.queue_entries;
    ledger_config.session_entries = usize::try_from(max_sessions)
        .unwrap_or(usize::MAX)
        .saturating_mul(2);
    ledger_config.checkpoint_interval = Duration::from_millis(config.ledger.checkpoint_interval_ms);
    ledger_config.write_ahead_budget = Duration::from_millis(limits.write_ahead_timeout_ms);
    let (writer, ledger_thread) = LedgerWriter::start(
        ledger_config,
        services.ledger_log,
        signer,
        Arc::new(MetricsObserver),
    )
    .context("start the ledger")?;
    let audit = LedgerAudit::new(writer);

    let census_producer = services.broker.producer("census")?;
    let events = EventPublisher::new(
        services.broker.producer("events")?,
        runtime.handle().clone(),
    );
    let node_states = Arc::new(NodeStates::new());
    let penalty_box = Arc::new(PenaltyBox::new(&limits)?);
    let provisioning_addresses = Arc::new(AddressSlots::new(
        limits.max_provisioning_connections_per_ip,
        &limits,
    )?);

    let credential_files_seen = credential_files_modified(&config);
    let mut provisioning_config = ProvisioningConfig::load(&ProvisioningFiles {
        instance: &config.instance,
        fleet_tokens_file: &config.provision.fleet_tokens_file,
        install_token_keys: &config.provision.install_token_keys,
        device_id_key_file: &config.provision.device_id_key_file,
        fleet_client_ca: &config.fleet.client_ca,
    })?;
    if !config
        .provision
        .tpm_endorsement_roots
        .as_os_str()
        .is_empty()
    {
        provisioning_config.endorsement_roots =
            load_certificates(&config.provision.tpm_endorsement_roots)?;
    }
    provisioning_config.challenge_ttl = Duration::from_millis(config.provision.challenge_ttl_ms);
    provisioning_config.renew_grace = config.renew_grace()?;
    provisioning_config.certificate_lifetime = config.certificate_lifetime()?;
    provisioning_config.enrollments_per_second = limits.enrollments_per_second;
    provisioning_config.enrollments_per_second_per_credential =
        limits.enrollments_per_second_per_credential;
    provisioning_config.enrollment_alert_per_minute = u64::from(limits.enrollment_alert_per_minute);
    let mut step_ca_config = StepCaConfig::load(
        &config.step_ca.url,
        &config.step_ca.root,
        &config.step_ca.provisioner,
        &config.step_ca.provisioner_key_file,
        config.certificate_lifetime()?,
    )?;
    step_ca_config.max_concurrent = config.step_ca.max_concurrent;
    step_ca_config.timeout = Duration::from_millis(config.step_ca.timeout_ms);
    let step_ca = StepCaClient::new(step_ca_config).context("create the step-ca client")?;
    let provisioning = Provisioning::new(
        provisioning_config,
        step_ca,
        Arc::new(EnrollmentPublisher {
            events: events.clone(),
            topic: config.kafka.topics.enrollments.clone(),
        }),
        node_states.clone(),
        penalty_box.clone(),
    )
    .map_err(|error| anyhow::anyhow!("start provisioning: {error}"))?;
    let fleet_client_roots = Arc::new(FleetClientRoots {
        verifier: fleet_clients.clone(),
        renewal: provisioning.trust(),
    });
    let provision_verifier = RenewClientVerifier::new(provisioning.trust());

    let tls = TlsConfigs {
        fleet: crate::tls::server_config(fleet_certificate.clone(), fleet_clients.clone(), false)?,
        provision: crate::tls::server_config(
            provision_certificate.clone(),
            provision_verifier,
            true,
        )?,
        inner: crate::tls::server_config(inner_certificate.clone(), inner_clients.clone(), false)?,
        admin: admin.as_ref().map(|admin| admin.server.clone()),
    };

    let (fleet_listener, provision_listener, inner_listener, relay_listener, admin_listener) = {
        let _entered = runtime.enter();
        let fleet = bind("fleet", &config.fleet.listen)?;
        let provision = if config.shares_fleet_port() {
            None
        } else {
            Some(bind("provision", &config.provision.listen)?)
        };
        let inner = bind("inner", &config.inner.listen)?;
        let relay = bind("relay", &config.inner.relay_listen)?;
        let admin = bind("admin", &config.admin.listen)?;
        (fleet, provision, inner, relay, admin)
    };
    let addresses = BoundAddresses {
        fleet: fleet_listener.local_addr()?,
        provision: match &provision_listener {
            Some(listener) => listener.local_addr()?,
            None => fleet_listener.local_addr()?,
        },
        inner: inner_listener.local_addr()?,
        relay: relay_listener.local_addr()?,
        admin: admin_listener.local_addr()?,
    };

    let shard_count = config.shard_count();
    let mut shard_receivers = Vec::with_capacity(shard_count);
    let mut handles = Vec::with_capacity(shard_count);
    for _ in 0..shard_count {
        let (handle, receivers) = ShardHandle::new();
        handles.push(handle);
        shard_receivers.push(receivers);
    }

    let instance_limits = InstanceLimits::new(&limits);
    let shared = Arc::new(Shared {
        directory: Arc::new(Mutex::new(Directory::new(&config.instance))),
        handshakes: HandshakeBucket::new(limits.handshakes_per_second),
        setups: SetupRate::new(limits.session_setups_per_identity_per_5s),
        relays: Arc::new(tokio::sync::Semaphore::new(
            limits.max_relayed_connections as usize,
        )),
        provisioning_connections: Arc::new(tokio::sync::Semaphore::new(
            limits.max_provisioning_connections as usize,
        )),
        provisioning_addresses,
        limits,
        max_sessions,
        instance_limits,
        node_states,
        intended_processes: Arc::new(IntendedProcesses::new(
            config.admission.max_intended_processes,
            i64::try_from(config.admission.clock_skew_ms).unwrap_or(i64::MAX),
            Duration::from_millis(config.admission.intent_wait_ms),
        )),
        permissions: ArcSwap::from_pointee(permissions),
        registry,
        audit,
        param_key: Arc::from(param_key),
        events,
        penalty_box,
        tls,
        provisioning,
        shards: handles,
        readiness: Readiness {
            topics: AtomicBool::new(topic_problems.is_empty()),
            ..Readiness::default()
        },
        accepting: CancellationToken::new(),
        accepting_inner: CancellationToken::new(),
        sessions: AtomicU64::new(0),
        started_at: now(),
        config,
    });

    let consumers = CancellationToken::new();
    let consumer_threads =
        crate::consumers::start(shared.clone(), services.broker.clone(), consumers.clone())?;

    let mut shard_threads = Vec::with_capacity(shard_count);
    for (index, receivers) in shard_receivers.into_iter().enumerate() {
        let listeners = ShardListeners {
            fleet: fleet_listener.try_clone()?,
            provision: provision_listener
                .as_ref()
                .map(std::net::TcpListener::try_clone)
                .transpose()?,
            inner: inner_listener.try_clone()?,
            relay: relay_listener.try_clone()?,
        };
        let shard_shared = shared.clone();
        shard_threads.push(
            std::thread::Builder::new()
                .name(format!("nightfall-shard-{index}"))
                .spawn(move || crate::shard::run(index, shard_shared, listeners, receivers))
                .context("start a shard thread")?,
        );
    }

    let background = CancellationToken::new();
    let census_stop = CancellationToken::new();
    let census = CensusProducer::new(
        CensusSettings {
            instance: shared.config.instance.clone(),
            topic: shared.config.kafka.topics.census.clone(),
            inner_address: shared.config.inner.advertise.clone(),
            relay_address: shared.config.inner.relay_advertise.clone(),
            heartbeat_seconds: shared.config.kafka.census_heartbeat_seconds,
            started_at: shared.started_at.clone(),
        },
        census_producer.clone(),
        shared.directory.clone(),
    );
    let census_task = runtime.spawn({
        let shared = shared.clone();
        let stop = census_stop.clone();
        async move {
            while !shared.readiness.directory.load(Ordering::Acquire) {
                tokio::select! {
                    () = stop.cancelled() => return census,
                    () = tokio::time::sleep(CENSUS_START_POLL) => {}
                }
            }
            crate::census::run(
                census,
                Duration::from_secs(shared.config.kafka.census_interval_seconds),
                Duration::from_secs(shared.config.kafka.census_heartbeat_seconds),
                stop,
            )
            .await
        }
    });

    let mut reloadables: Vec<Arc<dyn Reloadable>> = vec![
        fleet_certificate,
        provision_certificate,
        inner_certificate,
        fleet_client_roots,
        inner_clients,
    ];
    if let Some(admin) = admin {
        reloadables.extend(admin.reloadables);
    }
    runtime.spawn(crate::tls::reload_loop(
        reloadables,
        RELOAD_PERIOD,
        background.clone(),
    ));
    runtime.spawn(reload_permissions(shared.clone(), background.clone()));
    runtime.spawn(reload_credentials(
        shared.clone(),
        credential_files_seen,
        background.clone(),
    ));
    runtime.spawn(update_gauges(
        shared.clone(),
        services.broker.clone(),
        background.clone(),
    ));
    runtime.spawn(crate::admin::serve(
        shared.clone(),
        admin_listener,
        background.clone(),
    ));

    tracing::info!(
        instance = %shared.config.instance,
        shards = shard_count,
        max_sessions,
        fleet = %addresses.fleet,
        inner = %addresses.inner,
        relay = %addresses.relay,
        admin = %addresses.admin,
        "nightfall started"
    );
    Ok(Instance {
        shared,
        addresses,
        runtime: Some(runtime),
        shard_threads,
        ledger_thread: Some(ledger_thread),
        consumers,
        consumer_threads,
        census: Some(census_task),
        census_stop,
        background,
        census_producer,
    })
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

pub fn apply_permissions(shared: &Shared, permissions: Permissions) {
    let permissions = Arc::new(permissions);
    shared.permissions.store(permissions.clone());
    shared.broadcast(|| Control::Permissions(permissions.clone()));
}

async fn reload_permissions(shared: Arc<Shared>, stop: CancellationToken) {
    let path = shared.config.permissions.file.clone();
    let mut seen = modified(&path);
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            () = tokio::time::sleep(RELOAD_PERIOD) => {}
        }
        let now = modified(&path);
        if now == seen {
            continue;
        }
        match load_permissions(&path) {
            Ok(permissions) => {
                seen = now;
                tracing::info!(path = %path.display(), "permissions reloaded");
                apply_permissions(&shared, permissions);
            }
            Err(error) => {
                metrics::counter!("nightfall_permissions_reload_failures_total").increment(1);
                tracing::warn!(
                    path = %path.display(),
                    error = format!("{error:#}"),
                    "permissions changed but could not be loaded; the previous ones stay in use"
                );
            }
        }
    }
}

pub fn credential_files_modified(config: &Config) -> [Option<SystemTime>; 2] {
    [
        modified(&config.provision.fleet_tokens_file),
        modified(&config.provision.install_token_keys),
    ]
}

pub fn reload_credentials_if_changed(shared: &Shared, seen: &mut [Option<SystemTime>; 2]) {
    let now = credential_files_modified(&shared.config);
    let tokens_path = &shared.config.provision.fleet_tokens_file;
    if now[0] != seen[0] {
        match nightfall_provisioning::credential::FleetTokens::load(tokens_path) {
            Ok(tokens) => {
                seen[0] = now[0];
                shared.provisioning.replace_fleet_tokens(tokens);
            }
            Err(error) => {
                metrics::counter!("nightfall_credentials_reload_failures_total", "file" => "fleet_tokens_file").increment(1);
                tracing::warn!(
                    path = %tokens_path.display(),
                    %error,
                    "the fleet tokens changed but could not be loaded; the previous ones stay in use"
                );
            }
        }
    }
    let keys_path = &shared.config.provision.install_token_keys;
    if now[1] != seen[1] {
        match nightfall_provisioning::credential::InstallTokenKeys::load(keys_path) {
            Ok(keys) => {
                seen[1] = now[1];
                shared.provisioning.replace_install_token_keys(keys);
            }
            Err(error) => {
                metrics::counter!("nightfall_credentials_reload_failures_total", "file" => "install_token_keys").increment(1);
                tracing::warn!(
                    path = %keys_path.display(),
                    %error,
                    "the install token keys changed but could not be loaded; the previous ones stay in use"
                );
            }
        }
    }
}

async fn reload_credentials(
    shared: Arc<Shared>,
    mut seen: [Option<SystemTime>; 2],
    stop: CancellationToken,
) {
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            () = tokio::time::sleep(RELOAD_PERIOD) => {}
        }
        reload_credentials_if_changed(&shared, &mut seen);
    }
}

async fn update_gauges(shared: Arc<Shared>, broker: Arc<dyn Broker>, stop: CancellationToken) {
    let mut last_topic_check = Instant::now();
    let mut first = true;
    loop {
        if !first {
            tokio::select! {
                () = stop.cancelled() => return,
                () = tokio::time::sleep(GAUGE_PERIOD) => {}
            }
        }
        first = false;
        crate::admin::install_metrics().run_upkeep();
        metrics::gauge!("nightfall_ledger_queue_depth")
            .set(shared.audit.writer().queue_depth() as f64);
        metrics::gauge!("nightfall_penalty_box_ips")
            .set(shared.penalty_box.penalized_count() as f64);
        shared.provisioning.refresh_rate_alert(Instant::now());
        let remote = shared.directory().remote_count();
        metrics::gauge!("nightfall_directory_remote_nodes").set(remote as f64);
        metrics::gauge!("nightfall_ready").set(if shared.ready().is_ok() { 1.0 } else { 0.0 });
        if last_topic_check.elapsed() >= RELOAD_PERIOD {
            last_topic_check = Instant::now();
            let checking = broker.clone();
            let config = shared.config.clone();
            let problems =
                tokio::task::spawn_blocking(move || check_topics(checking.as_ref(), &config)).await;
            match problems {
                Ok(problems) => {
                    let was = shared
                        .readiness
                        .topics
                        .swap(problems.is_empty(), Ordering::AcqRel);
                    for problem in &problems {
                        tracing::error!(problem = %problem, "a Kafka topic is not ready");
                    }
                    if problems.is_empty() && !was {
                        tracing::info!("every Kafka topic is present");
                    }
                }
                Err(error) => tracing::error!(%error, "checking the Kafka topics failed"),
            }
        }
    }
}

impl Instance {
    pub fn handle(&self) -> tokio::runtime::Handle {
        self.runtime
            .as_ref()
            .expect("the control runtime lives until shutdown")
            .handle()
            .clone()
    }

    pub fn drain(&mut self, drain: Duration) {
        let shared = self.shared.clone();
        shared.readiness.draining.store(true, Ordering::Release);
        shared.accepting.cancel();
        tracing::info!(drain_seconds = drain.as_secs(), "draining");
        let mut sessions: Vec<(u64, usize, u64)> = shared
            .directory()
            .local_entries()
            .map(|(namespace_id, entry)| (*namespace_id, entry.shard, entry.epoch))
            .collect();
        use rand::seq::SliceRandom;
        sessions.shuffle(&mut rand::rng());
        let started = Instant::now();
        let count = sessions.len().max(1) as u32;
        for (position, (namespace_id, shard, epoch)) in sessions.into_iter().enumerate() {
            let due = drain.mul_f64(position as f64 / f64::from(count));
            if let Some(wait) = due.checked_sub(started.elapsed()) {
                std::thread::sleep(wait);
            }
            shared.send(
                shard,
                Control::Close {
                    namespace_id,
                    epoch,
                    reason: crate::events::DisconnectReason::Shutdown,
                },
            );
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while shared.sessions.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        tracing::info!(
            remaining = shared.sessions.load(Ordering::Acquire),
            "sessions closed"
        );
        self.census_stop.cancel();
        let handle = self.handle();
        if let Some(census) = self.census.take() {
            match handle
                .block_on(async { tokio::time::timeout(DRAIN_CENSUS_TIMEOUT, census).await })
            {
                Ok(Ok(mut producer)) => {
                    match handle.block_on(async {
                        tokio::time::timeout(DRAIN_CENSUS_TIMEOUT, producer.publish_final()).await
                    }) {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => tracing::error!(
                            error = format!("{error:#}"),
                            "the final census was not written"
                        ),
                        Err(_) => tracing::error!(
                            timeout_seconds = DRAIN_CENSUS_TIMEOUT.as_secs(),
                            "the final census was not written in time"
                        ),
                    }
                }
                Ok(Err(error)) => tracing::error!(%error, "the census task failed"),
                Err(_) => tracing::error!(
                    timeout_seconds = DRAIN_CENSUS_TIMEOUT.as_secs(),
                    "the census task did not stop in time; no final census is written"
                ),
            }
        }
        shared.accepting_inner.cancel();
    }

    pub fn shutdown(mut self) {
        self.shared.accepting.cancel();
        self.shared.accepting_inner.cancel();
        self.census_stop.cancel();
        self.background.cancel();
        self.consumers.cancel();
        self.shared.broadcast(|| Control::Stop);
        for thread in self.shard_threads.drain(..) {
            if thread.join().is_err() {
                tracing::error!("a shard thread panicked");
            }
        }
        for thread in self.consumer_threads.drain(..) {
            if thread.join().is_err() {
                tracing::error!("a consumer thread panicked");
            }
        }
        self.shared.events.flush(Duration::from_secs(10));
        self.census_producer.flush(Duration::from_secs(10));
        self.shared.audit.writer().shutdown();
        if let Some(thread) = self.ledger_thread.take() {
            thread.join();
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(5));
        }
        tracing::info!("nightfall stopped");
    }
}

pub async fn wait_for_stop(shared: Arc<Shared>) -> bool {
    let mut terminate =
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(terminate) => Some(terminate),
            Err(error) => {
                tracing::error!(%error, "SIGTERM cannot be watched; only SIGINT stops nightfall");
                None
            }
        };
    let mut fenced_check = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            signal = async {
                match terminate.as_mut() {
                    Some(terminate) => terminate.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                if signal.is_some() {
                    tracing::info!("SIGTERM received");
                    return false;
                }
            }
            interrupted = tokio::signal::ctrl_c() => {
                if let Err(error) = interrupted {
                    tracing::error!(%error, "SIGINT cannot be watched");
                } else {
                    tracing::info!("SIGINT received");
                }
                return false;
            }
            _ = fenced_check.tick() => {
                if shared.audit.writer().status() == LedgerStatus::Fenced {
                    tracing::error!("a newer nightfall with this instance id fenced the ledger; exiting");
                    return true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kafka::MemoryBroker;

    #[test]
    fn a_topic_whose_cleanup_policy_is_not_exactly_the_expected_one_is_a_problem() {
        let config = Config::default();
        let broker = MemoryBroker::new();
        broker.create_topic("dusk.connections", 3, "delete");
        broker.create_topic("dusk.census", 1, "compact");
        broker.create_topic("dusk.ledger", 3, "delete");
        broker.create_topic("dusk.enrollments", 3, "delete");
        broker.create_topic("dusk.node-state", 1, "compact, delete");
        broker.create_topic("dusk.intended-processes", 3, "compact");
        let problems = check_topics(&broker, &config);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].starts_with("dusk.node-state"), "{problems:?}");
        broker.create_topic("dusk.node-state", 1, "compact");
        broker.create_topic("dusk.ledger", 3, "compact");
        let problems = check_topics(&broker, &config);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].starts_with("dusk.ledger"), "{problems:?}");
        broker.create_topic("dusk.ledger", 3, "delete");
        assert!(check_topics(&broker, &config).is_empty());
        broker.create_topic("dusk.intended-processes", 3, "compact,delete");
        let problems = check_topics(&broker, &config);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].starts_with("dusk.intended-processes"),
            "{problems:?}"
        );
    }
}
