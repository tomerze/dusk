use super::keys::{EcdsaKey, LedgerKey, hex_key};
use super::pki::{Issued, Pki};
use capnp::capability::FromClientHook;
use capnp::message::ReaderOptions;
use capnp_rpc::rpc_twoparty_capnp::Side;
use capnp_rpc::{RpcSystem, twoparty};
use dusk_capnp::dusk_capnp::dusk;
use nightfall::config::Config;
use nightfall::kafka::{Broker, MemoryBroker, OutgoingRecord};
use nightfall::server::{Instance, Services};
use nightfall_ledger::memory::MemoryLog;
use nightfall_provisioning::provision_capnp;
use rustls::ClientConfig;
use rustls_pki_types::{CertificateDer, ServerName};
use serde_json::Value;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

pub const FLEET_NAME: &str = "fleet.dusk.test";
pub const PROVISION_NAME: &str = "provision.dusk.test";
pub const INNER_SUFFIX: &str = "inner.dusk.test";
pub const ADMIN_NAME: &str = "admin.dusk.test";
pub const DEVICE: &str = "00112233445566778899aabbccddeeff";
pub const INSTALLATION: &str = "ffeeddccbbaa99887766554433221100";
pub const OTHER_DEVICE: &str = "0123456789abcdef0123456789abcdef";
pub const FLEET_TOKEN: &str = "fleet token of the test fleet";
pub const CAMPAIGN: &str = "0192f3a4-5b6c-7d8e-9f01-23456789abcd";
pub const INTENT_PRINCIPAL: &str = "token:0192f3a4-1111-7d8e-9f01-23456789abcd";

pub const PERMISSIONS: &str = r#"
[[role]]
name = "dawn"
allow = ["Dusk.process", "Dusk.run", "Dusk.ps", "Dusk.kill", "Dusk.hostname", "Dusk.waitpid", "Dusk.time", "Dusk.programs", "Dusk.namespaceId", "Dusk.dusk", "Process.*", "Portal.*", "OutputPortal.*", "ShPortal.*", "KvsPortal.get", "KvsPortal.exists", "KvsPortal.scan", "LogsPortal.*", "CpPortal.*", "SignalBatch.Ack.ack"]
deny = ["Dusk.settime", "Dusk.fleetToken"]
reverse_allow = ["Stream.*", "Created.created", "Sink.*", "LogsArgs.Server.openStream", "LogsArgs.Stream.*", "CpArgs.Server.write", "CpArgs.Server.stat", "ShStop.stop"]
quarantine_override = false

[[role]]
name = "operator"
allow = ["Dusk.process", "Dusk.run", "Dusk.ps", "Dusk.kill", "Dusk.hostname", "Dusk.waitpid", "Dusk.time", "Dusk.programs", "Dusk.namespaceId", "Process.*", "Portal.*", "OutputPortal.*", "ShPortal.*"]
deny = ["Dusk.settime"]
reverse_allow = ["Stream.*", "Created.created", "ShStop.stop"]
admission_exempt = true

[[role]]
name = "quarantine"
allow = ["Dusk.hostname", "Dusk.programs", "Dusk.namespaceId", "Dusk.time"]
reverse_allow = []

[[role]]
name = "admin"

[[principal]]
name = "dawn-*"
roles = ["dawn"]

[[principal]]
name = "operator-*"
roles = ["operator"]

[[principal]]
name = "admin-*"
roles = ["admin"]
"#;

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub struct Environment {
    pub directory: TempDir,
    pub pki: Pki,
    pub provisioner: EcdsaKey,
    pub ledger_key: LedgerKey,
    pub broker: Arc<MemoryBroker>,
}

pub struct Paths {
    root: PathBuf,
}

impl Paths {
    pub fn file(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

impl Environment {
    pub fn new() -> Environment {
        let directory = tempfile::tempdir().unwrap();
        let pki = Pki::new();
        let provisioner = EcdsaKey::new("nightfall-provisioner");
        let ledger_key = LedgerKey::new();
        let root = directory.path();
        let fleet = pki.fleet_server.server(&[FLEET_NAME]);
        let provision = pki.fleet_server.server(&[PROVISION_NAME]);
        let inner = pki.internal.server(&[&format!("*.{INNER_SUFFIX}")]);
        let admin = pki.internal.server(&[ADMIN_NAME]);
        for (name, issued) in [
            ("fleet", &fleet),
            ("provision", &provision),
            ("inner", &inner),
            ("admin", &admin),
        ] {
            write(&root.join(format!("{name}.crt")), &issued.certificate_pem);
            write(&root.join(format!("{name}.key")), &issued.key_pem);
        }
        write(&root.join("fleet-client-ca.crt"), &pki.fleet_client.pem);
        write(&root.join("internal-ca.crt"), &pki.internal.pem);
        write(&root.join("fleet-server-ca.crt"), &pki.fleet_server.pem);
        write(&root.join("permissions.toml"), PERMISSIONS);
        write(&root.join("ledger-signing.key"), &ledger_key.pem());
        write(&root.join("ledger-param.key"), &hex_key(0x5a));
        write(&root.join("device-id.key"), &hex_key(0x3c));
        write(
            &root.join("fleet-tokens.toml"),
            &format!(
                "[[token]]\nname = \"test\"\nvalue_sha256 = \"{}\"\ntenant = \"acme\"\n",
                nightfall_ledger::entry::sha256_hex(FLEET_TOKEN.as_bytes())
            ),
        );
        write(&root.join("install-token-jwks.json"), "{\"keys\": []}");
        write(
            &root.join("provisioner.jwk"),
            &provisioner.private_jwk().to_string(),
        );
        write(&root.join("step-ca-root.crt"), &pki.fleet_client.pem);
        let broker = MemoryBroker::new();
        broker.create_topic("dusk.connections", 3, "delete");
        broker.create_topic("dusk.census", 1, "compact");
        broker.create_topic("dusk.ledger", 2, "delete");
        broker.create_topic("dusk.enrollments", 3, "delete");
        broker.create_topic("dusk.node-state", 1, "compact");
        broker.create_topic("dusk.intended-processes", 3, "compact");
        Environment {
            directory,
            pki,
            provisioner,
            ledger_key,
            broker,
        }
    }

    pub fn paths(&self) -> Paths {
        Paths {
            root: self.directory.path().to_path_buf(),
        }
    }

    pub fn config(&self, instance: &str, partition: i64) -> Config {
        let paths = self.paths();
        let inner_port = free_port();
        let relay_port = free_port();
        let mut config = Config {
            instance: instance.to_string(),
            shards: 2,
            drain_seconds: 1,
            ..Config::default()
        };
        config.fleet.listen = "127.0.0.1:0".to_string();
        config.fleet.server_names = vec![FLEET_NAME.to_string()];
        config.fleet.certificate = paths.file("fleet.crt");
        config.fleet.key = paths.file("fleet.key");
        config.fleet.client_ca = paths.file("fleet-client-ca.crt");
        config.fleet.session_setup_timeout_ms = 5_000;
        config.provision.listen = "127.0.0.1:0".to_string();
        config.provision.server_names = vec![PROVISION_NAME.to_string()];
        config.provision.certificate = paths.file("provision.crt");
        config.provision.key = paths.file("provision.key");
        config.provision.fleet_tokens_file = paths.file("fleet-tokens.toml");
        config.provision.install_token_keys = paths.file("install-token-jwks.json");
        config.provision.device_id_key_file = paths.file("device-id.key");
        config.inner.listen = format!("127.0.0.1:{inner_port}");
        config.inner.relay_listen = format!("127.0.0.1:{relay_port}");
        config.inner.advertise = format!("127.0.0.1:{inner_port}");
        config.inner.relay_advertise = format!("127.0.0.1:{relay_port}");
        config.inner.server_name_suffix = INNER_SUFFIX.to_string();
        config.inner.certificate = paths.file("inner.crt");
        config.inner.key = paths.file("inner.key");
        config.inner.client_ca = paths.file("internal-ca.crt");
        config.step_ca.url = "https://127.0.0.1:9".to_string();
        config.step_ca.root = paths.file("step-ca-root.crt");
        config.step_ca.provisioner_key_file = paths.file("provisioner.jwk");
        config.ledger.signing_key_file = paths.file("ledger-signing.key");
        config.ledger.param_key_file = paths.file("ledger-param.key");
        config.ledger.checkpoint_interval_ms = 100;
        config.ledger.partition = partition;
        config.kafka.allow_plaintext = true;
        config.kafka.census_interval_seconds = 2;
        config.kafka.census_heartbeat_seconds = 1;
        config.schemas.directory = PathBuf::from(nightfall_membrane::test_support::TREE_SCHEMAS);
        config.permissions.file = paths.file("permissions.toml");
        config.admin.listen = "127.0.0.1:0".to_string();
        config.validate().unwrap();
        config
    }

    pub fn with_admin_tls(&self, config: &mut Config) {
        let paths = self.paths();
        config.admin.certificate = paths.file("admin.crt");
        config.admin.key = paths.file("admin.key");
        config.admin.client_ca = paths.file("internal-ca.crt");
    }

    pub fn start(&self, config: Config) -> (Instance, MemoryLog) {
        let log = MemoryLog::new();
        let broker: Arc<dyn Broker> = Arc::new(self.broker.clone());
        let instance = nightfall::server::start(
            config,
            Services {
                broker,
                ledger_log: Box::new(log.clone()),
            },
        )
        .unwrap();
        wait_ready(&instance);
        (instance, log)
    }

    pub fn node_state(
        &self,
        device_id: &str,
        installation_id: Option<&str>,
        lifecycle: Option<&str>,
    ) {
        self.broker
            .produce(node_state_record(
                "dusk.node-state",
                device_id,
                installation_id,
                lifecycle,
            ))
            .unwrap();
    }

    pub fn intend(
        &self,
        installation_id: &str,
        pid: u64,
        max_commands: u32,
        default_shell_commands: u32,
    ) {
        self.broker
            .produce(intended_process_record(
                "dusk.intended-processes",
                installation_id,
                pid,
                max_commands,
                default_shell_commands,
            ))
            .unwrap();
    }

    pub fn unintend(&self, installation_id: &str, pid: u64) {
        self.broker
            .produce(OutgoingRecord {
                topic: "dusk.intended-processes".to_string(),
                partition: None,
                key: Some(format!("{DEVICE}/{installation_id}/{pid}")),
                payload: None,
            })
            .unwrap();
    }

    pub fn client_config(
        &self,
        roots: &CertificateDer<'static>,
        identity: &Issued,
    ) -> Arc<ClientConfig> {
        let mut store = rustls::RootCertStore::empty();
        store.add(roots.clone()).unwrap();
        Arc::new(
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .unwrap()
                .with_root_certificates(store)
                .with_client_auth_cert(vec![identity.certificate.clone()], identity.key())
                .unwrap(),
        )
    }

    pub fn node_identity(&self, device_id: &str, installation_id: &str) -> Arc<ClientConfig> {
        let issued = self
            .pki
            .fleet_client
            .node(device_id, installation_id, Some("acme"));
        self.client_config(&self.pki.fleet_server.certificate, &issued)
    }

    pub fn node_identity_until(
        &self,
        device_id: &str,
        installation_id: &str,
        not_after: time::OffsetDateTime,
    ) -> Arc<ClientConfig> {
        let issued = self.pki.fleet_client.node_until(
            device_id,
            installation_id,
            Some("acme"),
            Some(not_after),
        );
        self.client_config(&self.pki.fleet_server.certificate, &issued)
    }

    pub fn principal(&self, name: &str) -> Arc<ClientConfig> {
        let issued = self.pki.internal.principal(name);
        self.client_config(&self.pki.internal.certificate, &issued)
    }

    pub fn connections(&self) -> Vec<Value> {
        self.broker
            .records("dusk.connections")
            .into_iter()
            .filter_map(|record| record.payload)
            .map(|payload| serde_json::from_slice::<Value>(&payload).unwrap())
            .collect()
    }
}

pub fn wait_ready(instance: &Instance) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while let Err(reason) = instance.shared.ready() {
        assert!(
            Instant::now() < deadline,
            "nightfall did not become ready: {reason}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn intended_process_record(
    topic: &str,
    installation_id: &str,
    pid: u64,
    max_commands: u32,
    default_shell_commands: u32,
) -> OutgoingRecord {
    let now = time::OffsetDateTime::now_utc();
    let format = |moment: time::OffsetDateTime| nightfall_ledger::time::format(moment);
    let payload = serde_json::json!({
        "schema": "dusk.intended-processes/v1",
        "id": uuid::Uuid::now_v7().to_string(),
        "time": format(now),
        "pid": pid.to_string(),
        "device_id": DEVICE,
        "installation_id": installation_id,
        "campaign_id": CAMPAIGN,
        "action_kind": "run_script",
        "principal": INTENT_PRINCIPAL,
        "subject": format!("campaign:{CAMPAIGN}"),
        "created_at": format(now - time::Duration::seconds(1)),
        "expires_at": format(now + time::Duration::minutes(10)),
        "max_commands": max_commands,
        "default_shell_commands": default_shell_commands,
    });
    OutgoingRecord {
        topic: topic.to_string(),
        partition: None,
        key: Some(format!("{DEVICE}/{installation_id}/{pid}")),
        payload: Some(payload.to_string().into_bytes()),
    }
}

pub fn node_state_record(
    topic: &str,
    device_id: &str,
    installation_id: Option<&str>,
    lifecycle: Option<&str>,
) -> OutgoingRecord {
    let (scope, key) = match installation_id {
        Some(installation_id) => (
            "installation",
            format!("installation/{device_id}/{installation_id}"),
        ),
        None => ("device", format!("device/{device_id}")),
    };
    let payload = lifecycle.map(|lifecycle| {
        serde_json::json!({
            "schema": "dusk.node-state/v1",
            "id": uuid::Uuid::now_v7().to_string(),
            "time": nightfall_ledger::time::now(),
            "scope": scope,
            "device_id": device_id,
            "installation_id": installation_id,
            "lifecycle": lifecycle,
            "reason": "integration test",
            "actor": "twilight",
        })
        .to_string()
        .into_bytes()
    });
    OutgoingRecord {
        topic: topic.to_string(),
        partition: None,
        key: Some(key),
        payload,
    }
}

pub fn ledger(log: &MemoryLog) -> Vec<Value> {
    log.records()
        .iter()
        .map(|record| serde_json::from_slice(record).unwrap())
        .collect()
}

pub fn with_action<'a>(entries: &'a [Value], action: &str) -> Vec<&'a Value> {
    entries
        .iter()
        .filter(|entry| entry["action"] == action)
        .collect()
}

pub fn with_event<'a>(entries: &'a [Value], event: &str) -> Vec<&'a Value> {
    entries
        .iter()
        .filter(|entry| entry["event"] == event)
        .collect()
}

pub async fn wait_until(what: &str, timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + timeout;
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub fn local_session(instance: &Instance) -> Option<(u64, u64, usize)> {
    instance
        .shared
        .directory
        .lock()
        .unwrap()
        .local_entries()
        .next()
        .map(|(namespace_id, entry)| (*namespace_id, entry.epoch, entry.shard))
}

pub async fn wait_for_session(instance: &Instance) -> (u64, u64, usize) {
    let mut found = None;
    wait_until("a node session", Duration::from_secs(20), || {
        found = local_session(instance);
        found.is_some()
    })
    .await;
    found.unwrap()
}

async fn forward<From, To>(mut from: From, mut to: To, paused: Arc<AtomicBool>)
where
    From: AsyncRead + Unpin,
    To: AsyncWrite + Unpin,
{
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        let read = match from.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        while paused.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if to.write_all(&buffer[..read]).await.is_err() {
            return;
        }
    }
}

pub struct NodeBridge {
    pub paused: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for NodeBridge {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn link_node(
    fleet: SocketAddr,
    node_port: u16,
    identity: Arc<ClientConfig>,
) -> NodeBridge {
    link_node_with_header(fleet, node_port, identity, &[]).await
}

pub async fn link_node_with_header(
    fleet: SocketAddr,
    node_port: u16,
    identity: Arc<ClientConfig>,
    header: &[u8],
) -> NodeBridge {
    let mut tcp = TcpStream::connect(fleet).await.unwrap();
    tcp.set_nodelay(true).unwrap();
    tcp.write_all(header).await.unwrap();
    let tls = tokio_rustls::TlsConnector::from(identity)
        .connect(ServerName::try_from(FLEET_NAME).unwrap(), tcp)
        .await
        .unwrap();
    let node = TcpStream::connect(("127.0.0.1", node_port)).await.unwrap();
    node.set_nodelay(true).unwrap();
    let paused = Arc::new(AtomicBool::new(false));
    let (tls_reader, tls_writer) = tokio::io::split(tls);
    let (node_reader, node_writer) = node.into_split();
    let to_node = forward(tls_reader, node_writer, paused.clone());
    let to_nightfall = forward(node_reader, tls_writer, paused.clone());
    let task = tokio::task::spawn_local(async move {
        tokio::select! {
            () = to_node => {}
            () = to_nightfall => {}
        }
    });
    NodeBridge { paused, task }
}

pub struct Client {
    pub dusk: dusk::Client,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub fn anonymous_config(roots: &CertificateDer<'static>) -> Arc<ClientConfig> {
    let mut store = rustls::RootCertStore::empty();
    store.add(roots.clone()).unwrap();
    Arc::new(
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_root_certificates(store)
            .with_no_client_auth(),
    )
}

pub struct Provisioning {
    pub client: provision_capnp::provisioning::Client,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Provisioning {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn connect_provisioning(address: SocketAddr, config: Arc<ClientConfig>) -> Provisioning {
    let tcp = TcpStream::connect(address).await.unwrap();
    let tls = tokio_rustls::TlsConnector::from(config)
        .connect(ServerName::try_from(PROVISION_NAME).unwrap(), tcp)
        .await
        .unwrap();
    let (reader, writer) = tokio::io::split(tls);
    let network = twoparty::VatNetwork::new(
        reader.compat(),
        writer.compat_write(),
        Side::Client,
        ReaderOptions::new(),
    );
    let mut system = RpcSystem::new(Box::new(network), None);
    let client: provision_capnp::provisioning::Client = system.bootstrap(Side::Server);
    let task = tokio::task::spawn_local(async move {
        if let Err(error) = system.await {
            eprintln!("a provisioning link ended: {error}");
        }
    });
    Provisioning { client, task }
}

pub fn inner_name(namespace_id: u64) -> String {
    format!("{namespace_id:016x}.{INNER_SUFFIX}")
}

pub async fn connect_client(
    address: SocketAddr,
    namespace_id: u64,
    identity: Arc<ClientConfig>,
) -> Client {
    let tcp = TcpStream::connect(address).await.unwrap();
    tcp.set_nodelay(true).unwrap();
    let tls = tokio_rustls::TlsConnector::from(identity)
        .connect(ServerName::try_from(inner_name(namespace_id)).unwrap(), tcp)
        .await
        .unwrap();
    let (reader, writer) = tokio::io::split(tls);
    let network = twoparty::VatNetwork::new(
        reader.compat(),
        writer.compat_write(),
        Side::Client,
        ReaderOptions::new(),
    );
    let mut system = RpcSystem::new(Box::new(network), None);
    let bootstrap: capnp::capability::Client = system.bootstrap(Side::Server);
    let task = tokio::task::spawn_local(async move {
        if let Err(error) = system.await {
            eprintln!("an inner client link ended: {error}");
        }
    });
    Client {
        dusk: FromClientHook::new(bootstrap.hook.add_ref()),
        task,
    }
}

pub fn run<F: Future<Output = ()>>(test: F) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    tokio::task::LocalSet::new().block_on(&runtime, test);
}

pub fn run_returning<Output, F: Future<Output = Output>>(test: F) -> Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    tokio::task::LocalSet::new().block_on(&runtime, test)
}

pub fn shutdown(instance: Instance) {
    std::thread::spawn(move || instance.shutdown())
        .join()
        .unwrap();
}
