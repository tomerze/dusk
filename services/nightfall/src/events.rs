use crate::directory::{ConnectionEvent, NodeIdentity, namespace_hex, parse_namespace};
use crate::kafka::{OutgoingRecord, RecordProducer};
use nightfall_provisioning::events::{EnrollmentEvent, EnrollmentEvents};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use time::OffsetDateTime;

pub const CONNECTIONS_SCHEMA: &str = "dusk.connections/v1";

pub fn now() -> String {
    nightfall_ledger::time::now()
}

pub fn format_unix_milliseconds(milliseconds: i64) -> String {
    let moment = OffsetDateTime::from_unix_timestamp_nanos(i128::from(milliseconds) * 1_000_000)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH);
    nightfall_ledger::time::format(moment)
}

pub fn format_unix_seconds(seconds: i64) -> String {
    format_unix_milliseconds(seconds.saturating_mul(1000))
}

pub fn parse_time_milliseconds(text: &str) -> Option<i64> {
    OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|moment| (moment.unix_timestamp_nanos() / 1_000_000) as i64)
}

pub fn message_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisconnectReason {
    NodeClosed,
    TransportError,
    Replaced,
    Revoked,
    CertExpired,
    Shutdown,
    Killed,
    IdleTimeout,
}

impl DisconnectReason {
    pub fn as_str(self) -> &'static str {
        match self {
            DisconnectReason::NodeClosed => "node_closed",
            DisconnectReason::TransportError => "transport_error",
            DisconnectReason::Replaced => "replaced",
            DisconnectReason::Revoked => "revoked",
            DisconnectReason::CertExpired => "cert_expired",
            DisconnectReason::Shutdown => "shutdown",
            DisconnectReason::Killed => "killed",
            DisconnectReason::IdleTimeout => "idle_timeout",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRecord {
    pub identity: NodeIdentity,
    pub namespace_id: u64,
    pub epoch: u64,
    pub instance: String,
    pub inner_address: String,
    pub remote_address: String,
    pub tenant: Option<String>,
    pub cert_fingerprint: String,
    pub cert_not_after: String,
    pub connected_at: String,
}

impl SessionRecord {
    pub fn key(&self) -> String {
        format!(
            "{}/{}/{}",
            self.identity.device_id(),
            self.identity.installation_id(),
            namespace_hex(self.namespace_id)
        )
    }

    pub fn message(&self, disconnect: Option<DisconnectReason>) -> Value {
        json!({
            "schema": CONNECTIONS_SCHEMA,
            "id": message_id(),
            "time": now(),
            "event": if disconnect.is_some() { "disconnected" } else { "connected" },
            "device_id": self.identity.device_id(),
            "installation_id": self.identity.installation_id(),
            "namespace_id": namespace_hex(self.namespace_id),
            "epoch": self.epoch,
            "instance": self.instance,
            "inner_address": self.inner_address,
            "remote_address": self.remote_address,
            "tenant": self.tenant,
            "cert_fingerprint": self.cert_fingerprint,
            "cert_not_after": self.cert_not_after,
            "connected_at": self.connected_at,
            "disconnect_reason": disconnect.map(DisconnectReason::as_str),
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionMessage {
    schema: String,
    id: String,
    time: String,
    event: String,
    device_id: String,
    installation_id: String,
    namespace_id: String,
    epoch: u64,
    instance: String,
    inner_address: String,
    remote_address: String,
    tenant: Option<String>,
    cert_fingerprint: String,
    cert_not_after: String,
    connected_at: String,
    disconnect_reason: Option<String>,
}

const DISCONNECT_REASONS: [&str; 10] = [
    "node_closed",
    "transport_error",
    "replaced",
    "revoked",
    "cert_expired",
    "shutdown",
    "killed",
    "binding_conflict",
    "idle_timeout",
    "lifecycle_changed",
];

pub fn parse_connection(payload: &[u8]) -> Result<ConnectionEvent, String> {
    let message: ConnectionMessage =
        serde_json::from_slice(payload).map_err(|error| error.to_string())?;
    if message.schema != CONNECTIONS_SCHEMA {
        return Err(format!("schema {:?}", message.schema));
    }
    let connected = match (message.event.as_str(), message.disconnect_reason.as_deref()) {
        ("connected", None) => true,
        ("disconnected", Some(reason)) if DISCONNECT_REASONS.contains(&reason) => false,
        (event, reason) => return Err(format!("event {event:?} with reason {reason:?}")),
    };
    let identity = NodeIdentity::parse(&message.device_id, &message.installation_id)
        .ok_or("an invalid device or installation id")?;
    let namespace_id = parse_namespace(&message.namespace_id).ok_or("an invalid namespace id")?;
    if message.instance.is_empty() || message.instance.contains('/') {
        return Err(format!("instance {:?}", message.instance));
    }
    for (name, text) in [
        ("time", &message.time),
        ("cert_not_after", &message.cert_not_after),
        ("connected_at", &message.connected_at),
    ] {
        if parse_time_milliseconds(text).is_none() {
            return Err(format!("{name} {text:?}"));
        }
    }
    if uuid::Uuid::parse_str(&message.id).is_err()
        || message.cert_fingerprint.len() != 64
        || message.remote_address.is_empty()
        || message
            .tenant
            .as_ref()
            .is_some_and(|tenant| tenant.is_empty())
    {
        return Err("an invalid id, fingerprint, address or tenant".to_string());
    }
    Ok(ConnectionEvent {
        connected,
        instance: message.instance,
        inner_address: message.inner_address,
        identity,
        namespace_id,
        epoch: message.epoch,
    })
}

#[derive(Clone)]
pub struct EventPublisher {
    producer: Arc<dyn RecordProducer>,
    runtime: tokio::runtime::Handle,
}

impl EventPublisher {
    pub fn new(
        producer: Arc<dyn RecordProducer>,
        runtime: tokio::runtime::Handle,
    ) -> EventPublisher {
        EventPublisher { producer, runtime }
    }

    pub fn publish(&self, topic: &str, key: Option<String>, message: &Value) {
        let payload = match serde_json::to_vec(message) {
            Ok(payload) => payload,
            Err(error) => {
                tracing::error!(topic, %error, "an event could not be encoded and is lost");
                return;
            }
        };
        let delivery = self.producer.send(OutgoingRecord {
            topic: topic.to_string(),
            partition: None,
            key: key.clone(),
            payload: Some(payload),
        });
        let topic = topic.to_string();
        self.runtime.spawn(async move {
            if let Err(error) = delivery.await {
                metrics::counter!("nightfall_event_delivery_failures_total", "topic" => topic.clone())
                    .increment(1);
                tracing::error!(topic, key, %error, "an event was not delivered to Kafka");
            }
        });
    }

    pub fn flush(&self, timeout: std::time::Duration) {
        self.producer.flush(timeout);
    }
}

pub struct EnrollmentPublisher {
    pub events: EventPublisher,
    pub topic: String,
}

impl EnrollmentEvents for EnrollmentPublisher {
    fn record(&self, event: EnrollmentEvent) {
        metrics::counter!(
            "nightfall_enrollments_total",
            "operation" => event.operation.name(),
            "outcome" => event.outcome.name()
        )
        .increment(1);
        self.events
            .publish(&self.topic, event.key(), &event.message());
    }

    fn rate_alert(&self, _per_minute: u64, active: bool) {
        metrics::gauge!("nightfall_enrollment_rate_alert").set(if active { 1.0 } else { 0.0 });
    }

    fn device_id_collision(&self, _device_id: &str, _installations: usize, _addresses: usize) {
        metrics::counter!("device_id_collision").increment(1);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    pub(crate) fn contracts() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts/kafka")
    }

    pub(crate) fn validator(topic: &str) -> jsonschema::Validator {
        let path = contracts().join(format!("{topic}.schema.json"));
        let schema: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let base = format!("file://{}", path.canonicalize().unwrap().display());
        jsonschema::options()
            .with_base_uri(base)
            .build(&schema)
            .unwrap()
    }

    pub(crate) fn assert_valid(validator: &jsonschema::Validator, message: &Value) {
        let errors: Vec<String> = validator
            .iter_errors(message)
            .map(|error| format!("{} at {}", error, error.instance_path()))
            .collect();
        assert!(errors.is_empty(), "{message}: {errors:?}");
    }

    fn record() -> SessionRecord {
        SessionRecord {
            identity: NodeIdentity::parse(
                "00112233445566778899aabbccddeeff",
                "ffeeddccbbaa99887766554433221100",
            )
            .unwrap(),
            namespace_id: 0x0102,
            epoch: 1_791_000_000_000_001,
            instance: "nightfall-0".to_string(),
            inner_address: "nightfall-0.nightfall-inner.dusk.svc:8444".to_string(),
            remote_address: "[2001:db8::7]:40000".to_string(),
            tenant: Some("acme".to_string()),
            cert_fingerprint: "ab".repeat(32),
            cert_not_after: format_unix_seconds(1_791_600_000),
            connected_at: now(),
        }
    }

    #[test]
    fn connection_messages_follow_the_contract_and_read_back() {
        let validator = validator("dusk.connections");
        let record = record();
        assert_eq!(
            record.key(),
            "00112233445566778899aabbccddeeff/ffeeddccbbaa99887766554433221100/0000000000000102"
        );
        let connected = record.message(None);
        assert_valid(&validator, &connected);
        let event = parse_connection(connected.to_string().as_bytes()).unwrap();
        assert!(event.connected);
        assert_eq!(event.namespace_id, 0x0102);
        assert_eq!(event.epoch, record.epoch);
        for reason in [
            DisconnectReason::NodeClosed,
            DisconnectReason::TransportError,
            DisconnectReason::Replaced,
            DisconnectReason::Revoked,
            DisconnectReason::CertExpired,
            DisconnectReason::Shutdown,
            DisconnectReason::Killed,
            DisconnectReason::IdleTimeout,
        ] {
            let message = record.message(Some(reason));
            assert_valid(&validator, &message);
            assert!(
                !parse_connection(message.to_string().as_bytes())
                    .unwrap()
                    .connected
            );
        }
    }

    #[test]
    fn refuses_connection_messages_the_contract_refuses() {
        let validator = validator("dusk.connections");
        let mut message = record().message(None);
        message["disconnect_reason"] = json!("replaced");
        assert!(!validator.is_valid(&message));
        assert!(parse_connection(message.to_string().as_bytes()).is_err());
        let mut message = record().message(None);
        message["namespace_id"] = json!("102");
        assert!(parse_connection(message.to_string().as_bytes()).is_err());
        let mut message = record().message(None);
        message["unexpected"] = json!(1);
        assert!(parse_connection(message.to_string().as_bytes()).is_err());
        assert!(parse_connection(b"not json").is_err());
        let examples = contracts().join("examples/dusk.connections");
        for entry in std::fs::read_dir(examples).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let parsed = parse_connection(text.as_bytes());
            if name.starts_with("invalid-") {
                if parsed.is_ok() {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    assert!(!validator.is_valid(&value), "{name}");
                }
            } else {
                assert!(parsed.is_ok(), "{name}: {parsed:?}");
            }
        }
    }

    #[test]
    fn formats_and_parses_times_with_nanoseconds() {
        assert_eq!(
            format_unix_milliseconds(1_791_278_043_512),
            "2026-10-06T09:14:03.512000000Z"
        );
        assert_eq!(
            parse_time_milliseconds("2026-10-06T09:14:03.512000000Z"),
            Some(1_791_278_043_512)
        );
        assert!(parse_time_milliseconds("yesterday").is_none());
    }
}
