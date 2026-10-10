use std::net::SocketAddr;

use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;
use time::macros::format_description;

use crate::credential::CredentialKind;

pub const SCHEMA: &str = "dusk.enrollments/v1";
pub const MAXIMUM_REPORTED_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Assign,
    Enroll,
    Renew,
}

impl Operation {
    pub fn name(self) -> &'static str {
        match self {
            Operation::Assign => "assign",
            Operation::Enroll => "enroll",
            Operation::Renew => "renew",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Issued,
    Assigned,
    Denied,
    RateLimited,
    Error,
}

impl Outcome {
    pub fn name(self) -> &'static str {
        match self {
            Outcome::Issued => "issued",
            Outcome::Assigned => "assigned",
            Outcome::Denied => "denied",
            Outcome::RateLimited => "rate_limited",
            Outcome::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnrollmentEvent {
    pub operation: Operation,
    pub outcome: Outcome,
    pub reason: Option<String>,
    pub device_id: Option<String>,
    pub installation_id: Option<String>,
    pub tenant: Option<String>,
    pub credential_kind: CredentialKind,
    pub credential_ref: Option<String>,
    pub credential_issuer: Option<String>,
    pub hardware_fingerprint_hash: Option<String>,
    pub remote_address: String,
    pub cert_serial: Option<String>,
    pub cert_fingerprint: Option<String>,
    pub cert_not_after: Option<String>,
    pub dusk_version: Option<String>,
    #[serde(rename = "impl")]
    pub implementation: Option<String>,
    pub target_os: Option<String>,
    pub target_arch: Option<String>,
    pub hostname: Option<String>,
    pub instance: String,
}

fn format_timestamp(moment: OffsetDateTime) -> String {
    moment
        .to_offset(time::UtcOffset::UTC)
        .format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:9]Z"
        ))
        .expect("every UTC moment between years 0 and 9999 formats")
}

pub fn format_unix_seconds(seconds: i64) -> Option<String> {
    OffsetDateTime::from_unix_timestamp(seconds)
        .ok()
        .map(format_timestamp)
}

pub fn remote_address(address: SocketAddr) -> String {
    match address {
        SocketAddr::V4(address) => address.to_string(),
        SocketAddr::V6(address) => format!("[{}]:{}", address.ip(), address.port()),
    }
}

pub fn reported(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    let mut end = text.len().min(MAXIMUM_REPORTED_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(String::from(&text[..end]))
}

impl EnrollmentEvent {
    pub fn message(&self) -> Value {
        let mut message =
            serde_json::to_value(self).expect("an enrollment event always serializes");
        let members = message
            .as_object_mut()
            .expect("an enrollment event is an object");
        members.insert(String::from("schema"), Value::from(SCHEMA));
        members.insert(
            String::from("id"),
            Value::from(uuid::Uuid::now_v7().to_string()),
        );
        members.insert(
            String::from("time"),
            Value::from(format_timestamp(OffsetDateTime::now_utc())),
        );
        message
    }

    pub fn key(&self) -> Option<String> {
        match (&self.device_id, &self.installation_id) {
            (Some(device_id), Some(installation_id)) => {
                Some(format!("{device_id}/{installation_id}"))
            }
            _ => None,
        }
    }
}

pub trait EnrollmentEvents: Send + Sync {
    fn record(&self, event: EnrollmentEvent);
    fn rate_alert(&self, _per_minute: u64, _active: bool) {}
    fn device_id_collision(&self, _device_id: &str, _installations: usize, _addresses: usize) {}
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv6Addr};

    use super::*;

    #[test]
    fn formats_addresses_and_bounds_reported_text() {
        assert_eq!(
            remote_address("203.0.113.24:51730".parse().unwrap()),
            "203.0.113.24:51730"
        );
        let mapped = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0x24)),
            443,
        );
        assert_eq!(remote_address(mapped), "[2001:db8::24]:443");
        assert_eq!(reported(""), None);
        assert_eq!(reported(&"é".repeat(200)).unwrap().len(), 256);
        assert_eq!(
            format_unix_seconds(1_791_278_042).unwrap(),
            "2026-10-06T09:14:02.000000000Z"
        );
    }
}
