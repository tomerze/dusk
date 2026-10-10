use anyhow::{Context, bail};
use nightfall_membrane::limits::Limits;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_PATH: &str = "/etc/nightfall/nightfall.toml";
pub const ENVIRONMENT_PREFIX: &str = "NIGHTFALL__";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub instance: String,
    pub shards: usize,
    pub drain_seconds: u64,
    pub fleet: FleetConfig,
    pub provision: ProvisionConfig,
    pub inner: InnerConfig,
    pub step_ca: StepCaConfig,
    pub ledger: LedgerConfig,
    pub kafka: KafkaConfig,
    pub schemas: SchemasConfig,
    pub permissions: PermissionsConfig,
    pub limits: toml::Table,
    pub admin: AdminConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FleetConfig {
    pub listen: String,
    pub server_names: Vec<String>,
    pub certificate: PathBuf,
    pub key: PathBuf,
    pub client_ca: PathBuf,
    pub handshake_timeout_ms: u64,
    pub heartbeat_seconds: u64,
    pub session_setup_timeout_ms: u64,
    pub max_sessions: u64,
    pub proxy_protocol: bool,
    pub proxy_protocol_trusted_cidrs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProvisionConfig {
    pub listen: String,
    pub server_names: Vec<String>,
    pub certificate: PathBuf,
    pub key: PathBuf,
    pub fleet_tokens_file: PathBuf,
    pub install_token_keys: PathBuf,
    pub device_id_key_file: PathBuf,
    pub challenge_ttl_ms: u64,
    pub renew_grace: String,
    pub tpm_endorsement_roots: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InnerConfig {
    pub listen: String,
    pub relay_listen: String,
    pub advertise: String,
    pub relay_advertise: String,
    pub server_name_suffix: String,
    pub certificate: PathBuf,
    pub key: PathBuf,
    pub client_ca: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StepCaConfig {
    pub url: String,
    pub root: PathBuf,
    pub provisioner: String,
    pub provisioner_key_file: PathBuf,
    pub certificate_lifetime: String,
    pub max_concurrent: usize,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LedgerConfig {
    pub signing_key_file: PathBuf,
    pub param_key_file: PathBuf,
    pub checkpoint_interval_ms: u64,
    pub queue_entries: usize,
    pub partition: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Topics {
    pub connections: String,
    pub census: String,
    pub ledger: String,
    pub enrollments: String,
    pub node_state: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KafkaConfig {
    pub brokers: String,
    pub allow_plaintext: bool,
    pub properties: BTreeMap<String, String>,
    pub topics: Topics,
    pub census_interval_seconds: u64,
    pub census_heartbeat_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SchemasConfig {
    pub directory: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PermissionsConfig {
    pub file: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminConfig {
    pub listen: String,
    pub certificate: PathBuf,
    pub key: PathBuf,
    pub client_ca: PathBuf,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            instance: "nightfall-0".to_string(),
            shards: 0,
            drain_seconds: 300,
            fleet: FleetConfig::default(),
            provision: ProvisionConfig::default(),
            inner: InnerConfig::default(),
            step_ca: StepCaConfig::default(),
            ledger: LedgerConfig::default(),
            kafka: KafkaConfig::default(),
            schemas: SchemasConfig::default(),
            permissions: PermissionsConfig::default(),
            limits: toml::Table::new(),
            admin: AdminConfig::default(),
        }
    }
}

impl Default for FleetConfig {
    fn default() -> FleetConfig {
        FleetConfig {
            listen: "0.0.0.0:8443".to_string(),
            server_names: vec!["fleet.dusk.example".to_string()],
            certificate: PathBuf::from("/etc/nightfall/tls/fleet.crt"),
            key: PathBuf::from("/etc/nightfall/tls/fleet.key"),
            client_ca: PathBuf::from("/etc/nightfall/pki/fleet-client-ca.crt"),
            handshake_timeout_ms: 10_000,
            heartbeat_seconds: 30,
            session_setup_timeout_ms: 15_000,
            max_sessions: 100_000,
            proxy_protocol: false,
            proxy_protocol_trusted_cidrs: Vec::new(),
        }
    }
}

impl Default for ProvisionConfig {
    fn default() -> ProvisionConfig {
        ProvisionConfig {
            listen: "0.0.0.0:8443".to_string(),
            server_names: vec!["provision.dusk.example".to_string()],
            certificate: PathBuf::from("/etc/nightfall/tls/provision.crt"),
            key: PathBuf::from("/etc/nightfall/tls/provision.key"),
            fleet_tokens_file: PathBuf::from("/etc/nightfall/secrets/fleet-tokens.toml"),
            install_token_keys: PathBuf::from("/etc/nightfall/secrets/install-token-jwks.json"),
            device_id_key_file: PathBuf::from("/etc/nightfall/secrets/device-id.key"),
            challenge_ttl_ms: 300_000,
            renew_grace: "2160h".to_string(),
            tpm_endorsement_roots: PathBuf::new(),
        }
    }
}

impl Default for InnerConfig {
    fn default() -> InnerConfig {
        InnerConfig {
            listen: "0.0.0.0:8444".to_string(),
            relay_listen: "0.0.0.0:8445".to_string(),
            advertise: "nightfall-0.nightfall-inner.dusk.svc:8444".to_string(),
            relay_advertise: "nightfall-0.nightfall-inner.dusk.svc:8445".to_string(),
            server_name_suffix: "fleet.dusk.example".to_string(),
            certificate: PathBuf::from("/etc/nightfall/tls/inner.crt"),
            key: PathBuf::from("/etc/nightfall/tls/inner.key"),
            client_ca: PathBuf::from("/etc/nightfall/pki/internal-ca.crt"),
        }
    }
}

impl Default for StepCaConfig {
    fn default() -> StepCaConfig {
        StepCaConfig {
            url: "https://step-ca:9000".to_string(),
            root: PathBuf::from("/etc/nightfall/pki/fleet-client-root.crt"),
            provisioner: "nightfall".to_string(),
            provisioner_key_file: PathBuf::from("/etc/nightfall/secrets/provisioner.jwk"),
            certificate_lifetime: "168h".to_string(),
            max_concurrent: 16,
            timeout_ms: 10_000,
        }
    }
}

impl Default for LedgerConfig {
    fn default() -> LedgerConfig {
        LedgerConfig {
            signing_key_file: PathBuf::from("/etc/nightfall/secrets/ledger-signing.key"),
            param_key_file: PathBuf::from("/etc/nightfall/secrets/ledger-param.key"),
            checkpoint_interval_ms: 1000,
            queue_entries: 100_000,
            partition: -1,
        }
    }
}

impl Default for Topics {
    fn default() -> Topics {
        Topics {
            connections: "dusk.connections".to_string(),
            census: "dusk.census".to_string(),
            ledger: "dusk.ledger".to_string(),
            enrollments: "dusk.enrollments".to_string(),
            node_state: "dusk.node-state".to_string(),
        }
    }
}

impl Default for KafkaConfig {
    fn default() -> KafkaConfig {
        KafkaConfig {
            brokers: "kafka:9092".to_string(),
            allow_plaintext: false,
            properties: BTreeMap::new(),
            topics: Topics::default(),
            census_interval_seconds: 300,
            census_heartbeat_seconds: 15,
        }
    }
}

impl Default for SchemasConfig {
    fn default() -> SchemasConfig {
        SchemasConfig {
            directory: PathBuf::from("/usr/share/nightfall/schemas"),
        }
    }
}

impl Default for PermissionsConfig {
    fn default() -> PermissionsConfig {
        PermissionsConfig {
            file: PathBuf::from("/etc/nightfall/permissions.toml"),
        }
    }
}

impl Default for AdminConfig {
    fn default() -> AdminConfig {
        AdminConfig {
            listen: "0.0.0.0:9100".to_string(),
            certificate: PathBuf::new(),
            key: PathBuf::new(),
            client_ca: PathBuf::new(),
        }
    }
}

pub fn parse_duration(text: &str) -> anyhow::Result<Duration> {
    let mut rest = text.trim();
    if rest.is_empty() {
        bail!("an empty duration");
    }
    if rest == "0" {
        return Ok(Duration::ZERO);
    }
    let mut total = 0f64;
    while !rest.is_empty() {
        let number_length = rest
            .find(|character: char| !(character.is_ascii_digit() || character == '.'))
            .with_context(|| format!("the duration {text:?} ends without a unit"))?;
        if number_length == 0 {
            bail!("the duration {text:?} has a unit without a number");
        }
        let number: f64 = rest[..number_length]
            .parse()
            .with_context(|| format!("the duration {text:?} has an invalid number"))?;
        rest = &rest[number_length..];
        let unit_length = rest
            .find(|character: char| character.is_ascii_digit() || character == '.')
            .unwrap_or(rest.len());
        let seconds_per_unit = match &rest[..unit_length] {
            "ns" => 1e-9,
            "us" | "µs" => 1e-6,
            "ms" => 1e-3,
            "s" => 1.0,
            "m" => 60.0,
            "h" => 3600.0,
            unit => bail!("the duration {text:?} has the unit {unit:?}; use ns, us, ms, s, m or h"),
        };
        total += number * seconds_per_unit;
        rest = &rest[unit_length..];
    }
    if !total.is_finite() || total > u64::MAX as f64 {
        bail!("the duration {text:?} is out of range");
    }
    Ok(Duration::from_secs_f64(total))
}

fn guide<'a>(table: &'a toml::Table, segments: &[String]) -> Option<&'a toml::Value> {
    let (last, parents) = segments.split_last()?;
    let mut current = table;
    for segment in parents {
        current = current.get(segment)?.as_table()?;
    }
    current.get(last)
}

fn literal(text: &str) -> Option<toml::Value> {
    let document: toml::Table = toml::from_str(&format!("value = {text}")).ok()?;
    document.get("value").cloned()
}

fn override_value(
    variable: &str,
    text: &str,
    guide: Option<&toml::Value>,
) -> anyhow::Result<toml::Value> {
    let invalid = |kind: &str| anyhow::anyhow!("{variable} must hold {kind}");
    Ok(match guide {
        Some(toml::Value::String(_)) => toml::Value::String(text.to_string()),
        Some(toml::Value::Integer(_)) => {
            toml::Value::Integer(text.trim().parse().map_err(|_| invalid("an integer"))?)
        }
        Some(toml::Value::Float(_)) => {
            toml::Value::Float(text.trim().parse().map_err(|_| invalid("a number"))?)
        }
        Some(toml::Value::Boolean(_)) => {
            toml::Value::Boolean(text.trim().parse().map_err(|_| invalid("true or false"))?)
        }
        Some(toml::Value::Array(_)) => match literal(text) {
            Some(value @ toml::Value::Array(_)) => value,
            _ => return Err(invalid("a TOML array")),
        },
        Some(toml::Value::Table(_)) => match literal(text) {
            Some(value @ toml::Value::Table(_)) => value,
            _ => return Err(invalid("a TOML inline table")),
        },
        Some(toml::Value::Datetime(_)) | None => {
            literal(text).unwrap_or_else(|| toml::Value::String(text.to_string()))
        }
    })
}

fn apply_override(
    document: &mut toml::Table,
    defaults: &toml::Table,
    variable: &str,
    text: &str,
) -> anyhow::Result<()> {
    let Some(path) = variable.strip_prefix(ENVIRONMENT_PREFIX) else {
        return Ok(());
    };
    let segments: Vec<String> = path.split("__").map(str::to_lowercase).collect();
    if segments.iter().any(String::is_empty) {
        bail!("{variable} does not name a configuration key");
    }
    let (last, parents) = segments.split_last().expect("segments are not empty");
    let value = if parents == ["kafka", "properties"] {
        toml::Value::String(text.to_string())
    } else {
        let existing = guide(document, &segments).or_else(|| guide(defaults, &segments));
        override_value(variable, text, existing)?
    };
    let mut current = &mut *document;
    for segment in parents {
        let entry = current
            .entry(segment.clone())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        current = entry.as_table_mut().with_context(|| {
            format!("{variable} names a key inside {segment}, which is not a table")
        })?;
    }
    current.insert(last.clone(), value);
    Ok(())
}

fn check_listen(name: &str, address: &str) -> anyhow::Result<SocketAddr> {
    address
        .parse()
        .with_context(|| format!("{name} = {address:?} is not an ip:port address"))
}

fn check_host_port(name: &str, address: &str) -> anyhow::Result<()> {
    let (host, port) = address
        .rsplit_once(':')
        .with_context(|| format!("{name} = {address:?} is not host:port"))?;
    if host.is_empty() || port.parse::<u16>().is_err() {
        bail!("{name} = {address:?} is not host:port");
    }
    Ok(())
}

fn is_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn check_server_names(name: &str, names: &[String]) -> anyhow::Result<()> {
    if names.is_empty() {
        bail!("{name} is empty");
    }
    for server_name in names {
        if !is_server_name(server_name) {
            bail!("{name} holds {server_name:?}, which is not a lowercase DNS name");
        }
    }
    Ok(())
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read the configuration {}", path.display()))?;
        Config::parse(&text, std::env::vars())
            .with_context(|| format!("load the configuration {}", path.display()))
    }

    pub fn parse(
        text: &str,
        environment: impl IntoIterator<Item = (String, String)>,
    ) -> anyhow::Result<Config> {
        let mut document: toml::Table = toml::from_str(text).context("parse the TOML")?;
        let defaults = toml::Table::try_from(Config::default())?;
        let mut variables: Vec<(String, String)> = environment
            .into_iter()
            .filter(|(name, _)| name.starts_with(ENVIRONMENT_PREFIX))
            .collect();
        variables.sort();
        for (name, value) in &variables {
            apply_override(&mut document, &defaults, name, value)?;
        }
        let properties = document
            .get("kafka")
            .and_then(|kafka| kafka.get("properties"))
            .and_then(toml::Value::as_table);
        for (key, value) in properties.into_iter().flatten() {
            if !value.is_str() {
                bail!("kafka.properties.{key} must be a string");
            }
        }
        let config: Config = toml::Value::Table(document)
            .try_into()
            .context("read the configuration keys")?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.instance.is_empty() || self.instance.contains('/') {
            bail!(
                "instance = {:?} must be non-empty and hold no slash",
                self.instance
            );
        }
        let fleet = check_listen("fleet.listen", &self.fleet.listen)?;
        let provision = check_listen("provision.listen", &self.provision.listen)?;
        let inner = check_listen("inner.listen", &self.inner.listen)?;
        let relay = check_listen("inner.relay_listen", &self.inner.relay_listen)?;
        let admin = check_listen("admin.listen", &self.admin.listen)?;
        let distinct = [
            ("inner.listen", inner),
            ("inner.relay_listen", relay),
            ("admin.listen", admin),
        ];
        for (index, (name, address)) in distinct.iter().enumerate() {
            for (other_name, other) in distinct[index + 1..]
                .iter()
                .chain([("fleet.listen", fleet), ("provision.listen", provision)].iter())
            {
                if address.port() == other.port() && address.port() != 0 {
                    bail!(
                        "{name} and {other_name} use the same port {}",
                        address.port()
                    );
                }
            }
        }
        if fleet.port() == provision.port() && fleet != provision && fleet.port() != 0 {
            bail!(
                "fleet.listen and provision.listen share port {} on different addresses; give them the same address to share the port",
                fleet.port()
            );
        }
        check_server_names("fleet.server_names", &self.fleet.server_names)?;
        check_server_names("provision.server_names", &self.provision.server_names)?;
        if self.shares_fleet_port() {
            for name in &self.fleet.server_names {
                if self.provision.server_names.contains(name) {
                    bail!(
                        "{name:?} is in both fleet.server_names and provision.server_names on one port"
                    );
                }
            }
        }
        if !is_server_name(&self.inner.server_name_suffix) {
            bail!(
                "inner.server_name_suffix = {:?} is not a lowercase DNS name",
                self.inner.server_name_suffix
            );
        }
        check_host_port("inner.advertise", &self.inner.advertise)?;
        check_host_port("inner.relay_advertise", &self.inner.relay_advertise)?;
        for (name, value) in [
            (
                "fleet.handshake_timeout_ms",
                self.fleet.handshake_timeout_ms,
            ),
            ("fleet.heartbeat_seconds", self.fleet.heartbeat_seconds),
            (
                "fleet.session_setup_timeout_ms",
                self.fleet.session_setup_timeout_ms,
            ),
            ("fleet.max_sessions", self.fleet.max_sessions),
            (
                "provision.challenge_ttl_ms",
                self.provision.challenge_ttl_ms,
            ),
            ("step_ca.timeout_ms", self.step_ca.timeout_ms),
            (
                "ledger.checkpoint_interval_ms",
                self.ledger.checkpoint_interval_ms,
            ),
            (
                "kafka.census_interval_seconds",
                self.kafka.census_interval_seconds,
            ),
            (
                "kafka.census_heartbeat_seconds",
                self.kafka.census_heartbeat_seconds,
            ),
        ] {
            if value == 0 {
                bail!("{name} must be greater than 0");
            }
        }
        if self.kafka.census_heartbeat_seconds >= self.kafka.census_interval_seconds {
            bail!("kafka.census_heartbeat_seconds must be less than kafka.census_interval_seconds");
        }
        if self.step_ca.max_concurrent == 0 {
            bail!("step_ca.max_concurrent must be greater than 0");
        }
        if self.ledger.queue_entries < 2 {
            bail!("ledger.queue_entries must be at least 2");
        }
        if self.fleet.proxy_protocol && self.fleet.proxy_protocol_trusted_cidrs.is_empty() {
            bail!(
                "fleet.proxy_protocol = true needs fleet.proxy_protocol_trusted_cidrs to name the load balancers allowed to send the header"
            );
        }
        for cidr in &self.fleet.proxy_protocol_trusted_cidrs {
            crate::limits::Cidr::parse(cidr)
                .with_context(|| format!("fleet.proxy_protocol_trusted_cidrs holds {cidr:?}"))?;
        }
        if !self.step_ca.url.starts_with("https://") {
            bail!("step_ca.url = {:?} must be an https URL", self.step_ca.url);
        }
        if self.renew_grace()?.is_zero() {
            bail!("provision.renew_grace must be greater than 0");
        }
        if self.certificate_lifetime()?.is_zero() {
            bail!("step_ca.certificate_lifetime must be greater than 0");
        }
        self.ledger_partition()?;
        self.limits()?;
        self.check_kafka_security()?;
        let admin_files = [
            &self.admin.certificate,
            &self.admin.key,
            &self.admin.client_ca,
        ];
        let set = admin_files
            .iter()
            .filter(|path| !path.as_os_str().is_empty())
            .count();
        if set != 0 && set != admin_files.len() {
            bail!(
                "admin.certificate, admin.key and admin.client_ca are set together or not at all"
            );
        }
        let topics = [
            &self.kafka.topics.connections,
            &self.kafka.topics.census,
            &self.kafka.topics.ledger,
            &self.kafka.topics.enrollments,
            &self.kafka.topics.node_state,
        ];
        if topics.iter().any(|topic| topic.is_empty()) {
            bail!("every kafka.topics entry must name a topic");
        }
        Ok(())
    }

    fn check_kafka_security(&self) -> anyhow::Result<()> {
        if self.kafka.allow_plaintext {
            return Ok(());
        }
        let protocol = self
            .kafka
            .properties
            .get("security.protocol")
            .map(|protocol| protocol.to_ascii_lowercase())
            .unwrap_or_else(|| "plaintext".to_string());
        if protocol != "ssl" && protocol != "sasl_ssl" {
            bail!(
                "kafka.properties security.protocol is {protocol:?}; set it to ssl or sasl_ssl, or set kafka.allow_plaintext = true for development"
            );
        }
        Ok(())
    }

    pub fn shares_fleet_port(&self) -> bool {
        self.fleet.listen == self.provision.listen
    }

    pub fn admin_tls(&self) -> bool {
        !self.admin.certificate.as_os_str().is_empty()
    }

    pub fn shard_count(&self) -> usize {
        if self.shards > 0 {
            return self.shards;
        }
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
    }

    pub fn renew_grace(&self) -> anyhow::Result<Duration> {
        parse_duration(&self.provision.renew_grace).context("provision.renew_grace")
    }

    pub fn certificate_lifetime(&self) -> anyhow::Result<Duration> {
        parse_duration(&self.step_ca.certificate_lifetime).context("step_ca.certificate_lifetime")
    }

    pub fn ledger_partition(&self) -> anyhow::Result<u32> {
        match self.ledger.partition {
            -1 => {
                let ordinal = self
                    .instance
                    .rsplit_once('-')
                    .map(|(_, ordinal)| ordinal)
                    .filter(|ordinal| !ordinal.is_empty() && ordinal.bytes().all(|byte| byte.is_ascii_digit()))
                    .with_context(|| {
                        format!(
                            "ledger.partition = -1 takes the partition from the StatefulSet ordinal at the end of instance, and {:?} has none",
                            self.instance
                        )
                    })?;
                ordinal.parse().with_context(|| {
                    format!(
                        "the ordinal of instance {:?} is out of range",
                        self.instance
                    )
                })
            }
            partition if (0..=i64::from(i32::MAX)).contains(&partition) => Ok(partition as u32),
            partition => bail!("ledger.partition = {partition} must be -1 or a partition number"),
        }
    }

    pub fn limits(&self) -> anyhow::Result<Limits> {
        let limits: Limits = toml::Value::Table(self.limits.clone())
            .try_into()
            .context("read [limits]")?;
        for cidr in &limits.cidr {
            crate::limits::Cidr::parse(&cidr.cidr)
                .with_context(|| format!("[[limits.cidr]] cidr = {:?}", cidr.cidr))?;
        }
        for cidr in &limits.per_ip_exempt_cidrs {
            crate::limits::Cidr::parse(cidr)
                .with_context(|| format!("limits.per_ip_exempt_cidrs holds {cidr:?}"))?;
        }
        if limits.max_message_bytes < 8 {
            bail!("limits.max_message_bytes must be at least 8");
        }
        Ok(limits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPECIFICATION: &str = r#"
instance = "nightfall-0"
shards = 0
drain_seconds = 300

[fleet]
listen = "0.0.0.0:8443"
server_names = ["fleet.dusk.example"]
certificate = "/etc/nightfall/tls/fleet.crt"
key = "/etc/nightfall/tls/fleet.key"
client_ca = "/etc/nightfall/pki/fleet-client-ca.crt"
handshake_timeout_ms = 10000
heartbeat_seconds = 30
session_setup_timeout_ms = 15000
max_sessions = 100000
proxy_protocol = false
proxy_protocol_trusted_cidrs = []

[provision]
listen = "0.0.0.0:8443"
server_names = ["provision.dusk.example"]
certificate = "/etc/nightfall/tls/provision.crt"
key = "/etc/nightfall/tls/provision.key"
fleet_tokens_file = "/etc/nightfall/secrets/fleet-tokens.toml"
install_token_keys = "/etc/nightfall/secrets/install-token-jwks.json"
device_id_key_file = "/etc/nightfall/secrets/device-id.key"
challenge_ttl_ms = 300000
renew_grace = "2160h"
tpm_endorsement_roots = ""

[inner]
listen = "0.0.0.0:8444"
relay_listen = "0.0.0.0:8445"
advertise = "nightfall-0.nightfall-inner.dusk.svc:8444"
relay_advertise = "nightfall-0.nightfall-inner.dusk.svc:8445"
server_name_suffix = "fleet.dusk.example"
certificate = "/etc/nightfall/tls/inner.crt"
key = "/etc/nightfall/tls/inner.key"
client_ca = "/etc/nightfall/pki/internal-ca.crt"

[step_ca]
url = "https://step-ca:9000"
root = "/etc/nightfall/pki/fleet-client-root.crt"
provisioner = "nightfall"
provisioner_key_file = "/etc/nightfall/secrets/provisioner.jwk"
certificate_lifetime = "168h"
max_concurrent = 16
timeout_ms = 10000

[ledger]
signing_key_file = "/etc/nightfall/secrets/ledger-signing.key"
param_key_file = "/etc/nightfall/secrets/ledger-param.key"
checkpoint_interval_ms = 1000
queue_entries = 100000
partition = -1

[kafka]
brokers = "kafka:9092"
allow_plaintext = true
properties = {}
topics = { connections = "dusk.connections", census = "dusk.census", ledger = "dusk.ledger", enrollments = "dusk.enrollments", node_state = "dusk.node-state" }
census_interval_seconds = 300
census_heartbeat_seconds = 15

[schemas]
directory = "/usr/share/nightfall/schemas"

[permissions]
file = "/etc/nightfall/permissions.toml"

[limits]
handshakes_per_second = 2000
handshake_failures_per_ip_per_minute = 30
credential_failures_per_ip_per_hour = 20
penalty_seconds = 60
session_setups_per_identity_per_5s = 1
enrollments_per_second = 50
enrollments_per_second_per_credential = 10
enrollment_alert_per_minute = 600
calls_per_second_per_principal_per_node = 200
calls_per_second_instance = 200000
max_inflight_calls_per_session = 256
max_message_bytes = 4194304
max_inflight_bytes_per_session = 8388608
max_inflight_bytes_instance = 4294967296
max_live_caps_per_session = 10000
max_relayed_connections = 10000

[admin]
listen = "0.0.0.0:9100"
certificate = ""
key = ""
client_ca = ""
"#;

    fn no_environment() -> Vec<(String, String)> {
        Vec::new()
    }

    fn environment(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    fn error_of(text: &str, variables: Vec<(String, String)>) -> String {
        format!("{:#}", Config::parse(text, variables).unwrap_err())
    }

    #[test]
    fn reads_the_specification_example_as_the_defaults() {
        let config = Config::parse(SPECIFICATION, no_environment()).unwrap();
        let mut defaults = Config {
            limits: config.limits.clone(),
            ..Config::default()
        };
        defaults.kafka.allow_plaintext = true;
        assert_eq!(config, defaults);
        assert_eq!(config.limits().unwrap(), Limits::default());
        assert_eq!(config.ledger_partition().unwrap(), 0);
        assert_eq!(
            config.renew_grace().unwrap(),
            Duration::from_secs(2160 * 3600)
        );
        assert_eq!(
            config.certificate_lifetime().unwrap(),
            Duration::from_secs(168 * 3600)
        );
        assert!(config.shares_fleet_port());
        assert!(!config.admin_tls());
        assert!(config.shard_count() >= 1);
    }

    #[test]
    fn an_empty_file_takes_every_default_but_refuses_plaintext_kafka() {
        let error = error_of("", no_environment());
        assert!(error.contains("security.protocol"), "{error}");
        let config = Config::parse("[kafka]\nallow_plaintext = true\n", no_environment()).unwrap();
        assert_eq!(config.fleet, FleetConfig::default());
        assert_eq!(config.limits().unwrap(), Limits::default());
        let secured = Config::parse(
            "[kafka]\nproperties = { \"security.protocol\" = \"SASL_SSL\" }\n",
            no_environment(),
        )
        .unwrap();
        assert!(!secured.kafka.allow_plaintext);
    }

    #[test]
    fn the_environment_overrides_keys_with_their_types() {
        let config = Config::parse(
            SPECIFICATION,
            environment(&[
                ("NIGHTFALL__INSTANCE", "nightfall-7"),
                (
                    "NIGHTFALL__INNER__ADVERTISE",
                    "nightfall-7.nightfall-inner.dusk.svc:8444",
                ),
                ("NIGHTFALL__FLEET__MAX_SESSIONS", "250000"),
                ("NIGHTFALL__FLEET__PROXY_PROTOCOL", "true"),
                (
                    "NIGHTFALL__FLEET__PROXY_PROTOCOL_TRUSTED_CIDRS",
                    "[\"10.0.0.0/8\"]",
                ),
                (
                    "NIGHTFALL__FLEET__SERVER_NAMES",
                    "[\"fleet.a.example\", \"fleet.b.example\"]",
                ),
                ("NIGHTFALL__KAFKA__TOPICS__LEDGER", "audit.ledger"),
                (
                    "NIGHTFALL__KAFKA__PROPERTIES__SASL__MECHANISM",
                    "SCRAM-SHA-512",
                ),
                ("NIGHTFALL__LIMITS__PENALTY_SECONDS", "120"),
                ("NIGHTFALL__STEP_CA__URL", "https://ca.internal:9000"),
                ("OTHER__INSTANCE", "ignored"),
            ]),
        );
        let error = format!("{:#}", config.as_ref().err().unwrap());
        assert!(error.contains("properties"), "{error}");

        let config = Config::parse(
            SPECIFICATION,
            environment(&[
                ("NIGHTFALL__INSTANCE", "nightfall-7"),
                (
                    "NIGHTFALL__INNER__ADVERTISE",
                    "nightfall-7.nightfall-inner.dusk.svc:8444",
                ),
                ("NIGHTFALL__FLEET__MAX_SESSIONS", "250000"),
                ("NIGHTFALL__FLEET__PROXY_PROTOCOL", "true"),
                (
                    "NIGHTFALL__FLEET__PROXY_PROTOCOL_TRUSTED_CIDRS",
                    "[\"10.0.0.0/8\"]",
                ),
                (
                    "NIGHTFALL__FLEET__SERVER_NAMES",
                    "[\"fleet.a.example\", \"fleet.b.example\"]",
                ),
                ("NIGHTFALL__KAFKA__TOPICS__LEDGER", "audit.ledger"),
                ("NIGHTFALL__LIMITS__PENALTY_SECONDS", "120"),
                ("NIGHTFALL__STEP_CA__URL", "https://ca.internal:9000"),
                ("OTHER__INSTANCE", "ignored"),
            ]),
        )
        .unwrap();
        assert_eq!(config.instance, "nightfall-7");
        assert_eq!(config.ledger_partition().unwrap(), 7);
        assert_eq!(
            config.inner.advertise,
            "nightfall-7.nightfall-inner.dusk.svc:8444"
        );
        assert_eq!(config.fleet.max_sessions, 250_000);
        assert!(config.fleet.proxy_protocol);
        assert_eq!(
            config.fleet.server_names,
            vec!["fleet.a.example".to_string(), "fleet.b.example".to_string()]
        );
        assert_eq!(config.kafka.topics.ledger, "audit.ledger");
        assert_eq!(config.limits().unwrap().penalty_seconds, 120);
        assert_eq!(config.step_ca.url, "https://ca.internal:9000");
    }

    #[test]
    fn a_numeric_looking_string_stays_a_string() {
        let config = Config::parse(
            "instance = \"nightfall-0\"\n[kafka]\nallow_plaintext = true\n",
            environment(&[("NIGHTFALL__KAFKA__BROKERS", "9092")]),
        )
        .unwrap();
        assert_eq!(config.kafka.brokers, "9092");
    }

    #[test]
    fn a_kafka_property_is_always_a_string_and_never_printed() {
        let config = Config::parse(
            "instance = \"nightfall-0\"\n[kafka]\nallow_plaintext = true\n",
            environment(&[("NIGHTFALL__KAFKA__PROPERTIES__SASL.PASSWORD", "123456")]),
        )
        .unwrap();
        assert_eq!(config.kafka.properties["sasl.password"], "123456");
        let error = error_of(
            "instance = \"nightfall-0\"\n[kafka]\nallow_plaintext = true\nproperties = { \"sasl.password\" = 123456 }\n",
            no_environment(),
        );
        assert!(error.contains("kafka.properties.sasl.password"), "{error}");
        assert!(!error.contains("123456"), "{error}");
    }

    #[test]
    fn refuses_badly_typed_and_unknown_overrides() {
        let error = error_of(
            SPECIFICATION,
            environment(&[("NIGHTFALL__FLEET__MAX_SESSIONS", "many")]),
        );
        assert!(
            error.contains("NIGHTFALL__FLEET__MAX_SESSIONS must hold an integer"),
            "{error}"
        );
        assert!(!error.contains("many"), "{error}");
        let error = error_of(
            SPECIFICATION,
            environment(&[("NIGHTFALL__KAFKA__PROPERTIES", "{ sasl.password = secret")]),
        );
        assert!(!error.contains("secret"), "{error}");
        let error = error_of(
            SPECIFICATION,
            environment(&[("NIGHTFALL__FLEET__NO_SUCH_KEY", "1")]),
        );
        assert!(error.contains("no_such_key"), "{error}");
        let error = error_of(
            SPECIFICATION,
            environment(&[("NIGHTFALL__FLEET____LISTEN", "x")]),
        );
        assert!(
            error.contains("does not name a configuration key"),
            "{error}"
        );
        let error = error_of(
            SPECIFICATION,
            environment(&[("NIGHTFALL__LIMITS__NO_SUCH_LIMIT", "1")]),
        );
        assert!(error.contains("no_such_limit"), "{error}");
    }

    #[test]
    fn refuses_unknown_keys_in_the_file() {
        let error = error_of(
            &format!("{SPECIFICATION}\n[extra]\nkey = 1\n"),
            no_environment(),
        );
        assert!(error.contains("extra"), "{error}");
    }

    #[test]
    fn validates_addresses_names_and_ports() {
        let cases = [
            ("[fleet]\nlisten = \"nowhere\"", "fleet.listen"),
            ("[inner]\nlisten = \"0.0.0.0:9100\"", "same port 9100"),
            ("[inner]\nrelay_listen = \"0.0.0.0:8444\"", "same port 8444"),
            (
                "[provision]\nlisten = \"127.0.0.1:8443\"",
                "share port 8443",
            ),
            ("[fleet]\nserver_names = []", "fleet.server_names is empty"),
            (
                "[fleet]\nserver_names = [\"Fleet.Example\"]",
                "lowercase DNS name",
            ),
            (
                "[provision]\nserver_names = [\"fleet.dusk.example\"]",
                "in both fleet.server_names and provision.server_names",
            ),
            ("[inner]\nadvertise = \"no-port\"", "inner.advertise"),
            (
                "[inner]\nserver_name_suffix = \"\"",
                "inner.server_name_suffix",
            ),
            ("instance = \"a/b\"", "no slash"),
            (
                "[fleet]\nheartbeat_seconds = 0",
                "fleet.heartbeat_seconds must be greater than 0",
            ),
            (
                "[kafka]\ncensus_heartbeat_seconds = 300",
                "less than kafka.census_interval_seconds",
            ),
            ("[step_ca]\nurl = \"http://step-ca:9000\"", "https"),
            (
                "[provision]\nrenew_grace = \"90 days\"",
                "provision.renew_grace",
            ),
            ("[step_ca]\ncertificate_lifetime = \"0h\"", "greater than 0"),
            ("[admin]\ncertificate = \"/admin.crt\"", "set together"),
            (
                "[fleet]\nproxy_protocol_trusted_cidrs = [\"10.0.0.0/33\"]",
                "proxy_protocol_trusted_cidrs",
            ),
            (
                "[fleet]\nproxy_protocol = true",
                "needs fleet.proxy_protocol_trusted_cidrs",
            ),
            (
                "[limits]\nper_ip_exempt_cidrs = [\"bogus\"]",
                "per_ip_exempt_cidrs",
            ),
            ("[kafka.topics]\nledger = \"\"", "every kafka.topics entry"),
        ];
        for (fragment, expected) in cases {
            let text = if fragment.starts_with('[') {
                format!("[kafka]\nallow_plaintext = true\n{fragment}\n").replace(
                    "[kafka]\nallow_plaintext = true\n[kafka]",
                    "[kafka]\nallow_plaintext = true",
                )
            } else {
                format!("{fragment}\n[kafka]\nallow_plaintext = true\n")
            };
            let error = error_of(&text, no_environment());
            assert!(error.contains(expected), "{fragment}: {error}");
        }
    }

    #[test]
    fn derives_the_ledger_partition_from_the_ordinal() {
        let parse = |instance: &str, partition: i64| {
            Config::parse(
                &format!(
                    "instance = \"{instance}\"\n[ledger]\npartition = {partition}\n[kafka]\nallow_plaintext = true\n"
                ),
                Vec::new(),
            )
        };
        assert_eq!(
            parse("nightfall-12", -1)
                .unwrap()
                .ledger_partition()
                .unwrap(),
            12
        );
        assert_eq!(parse("edge", 3).unwrap().ledger_partition().unwrap(), 3);
        let error = format!("{:#}", parse("edge", -1).unwrap_err());
        assert!(error.contains("StatefulSet ordinal"), "{error}");
        let error = format!("{:#}", parse("nightfall-0", -2).unwrap_err());
        assert!(
            error.contains("must be -1 or a partition number"),
            "{error}"
        );
    }

    #[test]
    fn parses_go_durations() {
        assert_eq!(
            parse_duration("168h").unwrap(),
            Duration::from_secs(168 * 3600)
        );
        assert_eq!(parse_duration("1h30m").unwrap(), Duration::from_secs(5400));
        assert_eq!(parse_duration("1.5s").unwrap(), Duration::from_millis(1500));
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
        assert_eq!(parse_duration("0").unwrap(), Duration::ZERO);
        assert!(parse_duration("").is_err());
        assert!(parse_duration("10").is_err());
        assert!(parse_duration("h").is_err());
        assert!(parse_duration("3d").is_err());
    }
}
