use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::canonical::{CanonicalError, LARGEST_EXACT_INTEGER, canonical_bytes};

pub const SCHEMA: &str = "dusk.ledger/v1";
pub const MAXIMUM_ENTRY_BYTES: usize = 262_144;
const FIXED_FIELD_BYTES: usize = 2048;
pub const CHAIN_START_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";
pub const NIGHTFALL_PRINCIPAL: &str = "nightfall";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Call,
    Result,
    Event,
    Checkpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    ClientToNode,
    NodeToClient,
    NightfallToNode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResultCode {
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "error:failed")]
    Failed,
    #[serde(rename = "error:overloaded")]
    Overloaded,
    #[serde(rename = "error:disconnected")]
    Disconnected,
    #[serde(rename = "error:unimplemented")]
    Unimplemented,
    #[serde(rename = "denied")]
    Denied,
    #[serde(rename = "rate_limited")]
    RateLimited,
    #[serde(rename = "revoked")]
    Revoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    SessionOpen,
    SessionClose,
    SessionRefused,
    SetupCall,
    MembraneDropped,
    BindingConflict,
    RpcRejected,
    ChainLink,
    ChainResumed,
    QuarantineOverride,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamField {
    pub name: String,
    pub redacted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntryContent {
    pub kind: Kind,
    pub device_id: Option<String>,
    pub installation_id: Option<String>,
    pub namespace_id: Option<u64>,
    pub epoch: Option<u64>,
    pub principal: String,
    pub pid: u64,
    pub session_id: Option<String>,
    pub call_id: Option<String>,
    pub cap_id: Option<u64>,
    pub parent_cap_id: Option<u64>,
    pub direction: Option<Direction>,
    pub action: Option<String>,
    pub interface_id: Option<u64>,
    pub method_id: Option<u16>,
    pub param_fields: Vec<ParamField>,
    pub param_cap_ids: Vec<u64>,
    pub param_hash: String,
    pub result_code: Option<ResultCode>,
    pub result_cap_ids: Vec<u64>,
    pub event: Option<Event>,
    pub event_detail: Option<Map<String, Value>>,
}

impl EntryContent {
    pub fn event(event: Event, principal: &str) -> EntryContent {
        EntryContent {
            event: Some(event),
            ..EntryContent::blank(Kind::Event, principal)
        }
    }

    pub fn blank(kind: Kind, principal: &str) -> EntryContent {
        EntryContent {
            kind,
            device_id: None,
            installation_id: None,
            namespace_id: None,
            epoch: None,
            principal: String::from(principal),
            pid: 0,
            session_id: None,
            call_id: None,
            cap_id: None,
            parent_cap_id: None,
            direction: None,
            action: None,
            interface_id: None,
            method_id: None,
            param_fields: Vec::new(),
            param_cap_ids: Vec::new(),
            param_hash: String::new(),
            result_code: None,
            result_cap_ids: Vec::new(),
            event: None,
            event_detail: None,
        }
    }

    pub fn validate(&self) -> Result<(), InvalidEntry> {
        let invalid = |reason: &str| Err(InvalidEntry(String::from(reason)));
        let is_hex = |text: &str, length: usize| {
            text.len() == length
                && text
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        };
        if let Some(device_id) = &self.device_id
            && !is_hex(device_id, 32)
        {
            return invalid("device_id is not 32 lowercase hex digits");
        }
        if let Some(installation_id) = &self.installation_id
            && !is_hex(installation_id, 32)
        {
            return invalid("installation_id is not 32 lowercase hex digits");
        }
        if self.principal.is_empty() {
            return invalid("principal is empty");
        }
        for (name, identifier) in [("session_id", &self.session_id), ("call_id", &self.call_id)] {
            if let Some(identifier) = identifier
                && !is_canonical_uuid(identifier)
            {
                return Err(InvalidEntry(format!(
                    "{name} is not a canonical lowercase UUID"
                )));
            }
        }
        if !self.param_hash.is_empty() && !is_hex(&self.param_hash, 64) {
            return invalid("param_hash is neither empty nor 64 lowercase hex digits");
        }
        if let Some(action) = &self.action
            && !is_valid_action(action)
        {
            return invalid(
                "action is neither Interface.method nor unknown:<interface id>.<method id>",
            );
        }
        if self.param_fields.iter().any(|field| field.name.is_empty()) {
            return invalid("a param field has an empty name");
        }
        let detail_bytes = match &self.event_detail {
            Some(detail) => {
                let detail = Value::Object(detail.clone());
                match canonical_bytes(&detail) {
                    Ok(canonical) if is_plain(&detail) => canonical.len(),
                    _ => {
                        return invalid(
                            "event_detail holds a member name that is not ASCII or a number that is not an integer between -2^53 and 2^53",
                        );
                    }
                }
            }
            None => 0,
        };
        let integers = [self.epoch, self.cap_id, self.parent_cap_id]
            .into_iter()
            .flatten()
            .chain(self.param_cap_ids.iter().copied())
            .chain(self.result_cap_ids.iter().copied());
        for number in integers {
            if number > LARGEST_EXACT_INTEGER {
                return Err(InvalidEntry(format!(
                    "{number} is above 2^53, which a JSON number does not hold exactly"
                )));
            }
        }
        let text_bytes = self.principal.len()
            + self.action.as_ref().map_or(0, String::len)
            + self
                .param_fields
                .iter()
                .map(|field| field.name.len())
                .sum::<usize>();
        let largest = FIXED_FIELD_BYTES
            + 6 * text_bytes
            + 32 * self.param_fields.len()
            + 17 * (self.param_cap_ids.len() + self.result_cap_ids.len())
            + detail_bytes;
        if largest > MAXIMUM_ENTRY_BYTES {
            return Err(InvalidEntry(format!(
                "the entry may take up to {largest} bytes, more than the {MAXIMUM_ENTRY_BYTES} a ledger entry may take"
            )));
        }
        let unknown_action = self
            .action
            .as_deref()
            .is_some_and(|action| action.starts_with("unknown:"));
        if unknown_action
            && (self.kind != Kind::Call
                || self.result_code != Some(ResultCode::Denied)
                || !self.param_hash.is_empty())
        {
            return invalid(
                "a call on an unknown interface is a denied call entry with an empty param_hash",
            );
        }
        match self.kind {
            Kind::Checkpoint => return invalid("checkpoints are written by the ledger itself"),
            Kind::Call | Kind::Result => {
                if self.event.is_some() || self.event_detail.is_some() {
                    return invalid("call and result entries carry no event");
                }
                let complete = self.device_id.is_some()
                    && self.installation_id.is_some()
                    && self.namespace_id.is_some()
                    && self.epoch.is_some()
                    && self.session_id.is_some()
                    && self.call_id.is_some()
                    && self.cap_id.is_some()
                    && self.direction.is_some()
                    && self.action.is_some()
                    && self.interface_id.is_some()
                    && self.method_id.is_some();
                if !complete {
                    return invalid(
                        "call and result entries need device_id, installation_id, namespace_id, epoch, session_id, call_id, cap_id, direction, action, interface_id and method_id",
                    );
                }
            }
            Kind::Event => {
                if self.event.is_none() {
                    return invalid("an event entry needs an event");
                }
                if !self.param_hash.is_empty() {
                    return invalid("event entries carry an empty param_hash");
                }
            }
        }
        if self.kind == Kind::Call {
            match self.result_code {
                None | Some(ResultCode::RateLimited) => {
                    if self.param_hash.is_empty() {
                        return invalid("a call entry that is not denied needs a param_hash");
                    }
                }
                Some(ResultCode::Denied) => {}
                Some(_) => {
                    return invalid(
                        "a call entry carries no result_code but denied or rate_limited",
                    );
                }
            }
        }
        if self.kind == Kind::Result {
            if !self.param_hash.is_empty() {
                return invalid("result entries carry an empty param_hash");
            }
            if matches!(
                self.result_code,
                None | Some(ResultCode::Denied) | Some(ResultCode::RateLimited)
            ) {
                return invalid("a result entry needs ok, error:* or revoked");
            }
        }
        match self.event {
            Some(Event::SetupCall) => {
                if self.principal != NIGHTFALL_PRINCIPAL
                    || self.direction != Some(Direction::NightfallToNode)
                    || self.action.is_none()
                    || self.interface_id.is_none()
                    || self.method_id.is_none()
                    || self.session_id.is_some()
                {
                    return invalid(
                        "a setup_call event is principal nightfall, direction nightfall_to_node, with action, interface_id and method_id and no session_id",
                    );
                }
            }
            Some(Event::SessionOpen) | Some(Event::SessionClose) => {
                if self.device_id.is_none()
                    || self.installation_id.is_none()
                    || self.namespace_id.is_none()
                    || self.epoch.is_none()
                    || self.session_id.is_some()
                    || self.call_id.is_some()
                {
                    return invalid(
                        "session_open and session_close carry device_id, installation_id, namespace_id and epoch and no session_id or call_id",
                    );
                }
            }
            Some(Event::SessionRefused) => {
                if self.device_id.is_none()
                    || self.installation_id.is_none()
                    || self.epoch.is_some()
                    || self.session_id.is_some()
                    || self.call_id.is_some()
                {
                    return invalid(
                        "session_refused carries device_id and installation_id and no epoch, session_id or call_id",
                    );
                }
            }
            Some(Event::QuarantineOverride) => {
                if self.session_id.is_none() || self.call_id.is_none() {
                    return invalid(
                        "quarantine_override carries the session_id and call_id of its call",
                    );
                }
            }
            Some(Event::MembraneDropped) => {
                if self.session_id.is_none() || self.call_id.is_some() {
                    return invalid("membrane_dropped carries a session_id and no call_id");
                }
            }
            Some(Event::BindingConflict) => {
                if self.epoch.is_some() || self.session_id.is_some() || self.call_id.is_some() {
                    return invalid("binding_conflict carries no epoch, session_id or call_id");
                }
            }
            Some(Event::RpcRejected) => {
                if self.call_id.is_some() {
                    return invalid("rpc_rejected carries no call_id");
                }
            }
            Some(Event::ChainLink) | Some(Event::ChainResumed) => {
                return invalid("chain_link and chain_resumed are written by the ledger itself");
            }
            None => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid ledger entry: {0}")]
pub struct InvalidEntry(pub String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerEntry {
    pub schema: String,
    pub id: String,
    pub time: String,
    pub kind: Kind,
    pub instance: String,
    pub partition: u32,
    pub sequence: u64,
    pub previous_hash: String,
    pub hash: String,
    pub device_id: Option<String>,
    pub installation_id: Option<String>,
    pub namespace_id: Option<String>,
    pub epoch: Option<u64>,
    pub principal: String,
    pub pid: String,
    pub session_id: Option<String>,
    pub call_id: Option<String>,
    pub cap_id: Option<u64>,
    pub parent_cap_id: Option<u64>,
    pub direction: Option<Direction>,
    pub action: Option<String>,
    pub interface_id: Option<String>,
    pub method_id: Option<u16>,
    pub param_fields: Vec<ParamField>,
    pub param_cap_ids: Vec<u64>,
    pub param_hash: String,
    pub result_code: Option<ResultCode>,
    pub result_cap_ids: Vec<u64>,
    pub event: Option<Event>,
    pub event_detail: Option<Map<String, Value>>,
    pub key_id: Option<String>,
    pub signature: Option<String>,
}

pub struct ChainFields<'a> {
    pub id: String,
    pub time: String,
    pub instance: &'a str,
    pub partition: u32,
    pub sequence: u64,
    pub previous_hash: String,
}

impl LedgerEntry {
    pub fn from_content(content: EntryContent, chain: ChainFields<'_>) -> LedgerEntry {
        LedgerEntry {
            schema: String::from(SCHEMA),
            id: chain.id,
            time: chain.time,
            kind: content.kind,
            instance: String::from(chain.instance),
            partition: chain.partition,
            sequence: chain.sequence,
            previous_hash: chain.previous_hash,
            hash: String::new(),
            device_id: content.device_id,
            installation_id: content.installation_id,
            namespace_id: content
                .namespace_id
                .map(|namespace_id| format!("{namespace_id:016x}")),
            epoch: content.epoch,
            principal: content.principal,
            pid: content.pid.to_string(),
            session_id: content.session_id,
            call_id: content.call_id,
            cap_id: content.cap_id,
            parent_cap_id: content.parent_cap_id,
            direction: content.direction,
            action: content.action,
            interface_id: content
                .interface_id
                .map(|interface_id| format!("{interface_id:016x}")),
            method_id: content.method_id,
            param_fields: content.param_fields,
            param_cap_ids: content.param_cap_ids,
            param_hash: content.param_hash,
            result_code: content.result_code,
            result_cap_ids: content.result_cap_ids,
            event: content.event,
            event_detail: content.event_detail,
            key_id: None,
            signature: None,
        }
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("a ledger entry always serializes to JSON")
    }

    pub fn compute_hash(&self) -> Result<String, CanonicalError> {
        entry_hash(&self.to_value())
    }

    pub fn seal(&mut self) -> Result<(), CanonicalError> {
        self.hash = self.compute_hash()?;
        Ok(())
    }

    pub fn payload(&self) -> Result<Vec<u8>, CanonicalError> {
        canonical_bytes(&self.to_value())
    }
}

pub fn entry_hash(entry: &Value) -> Result<String, CanonicalError> {
    let mut without_hash = entry.clone();
    if let Value::Object(members) = &mut without_hash {
        members.remove("hash");
    }
    Ok(sha256_hex(&canonical_bytes(&without_hash)?))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn is_canonical_uuid(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes
            .iter()
            .enumerate()
            .all(|(position, byte)| match position {
                8 | 13 | 18 | 23 => *byte == b'-',
                _ => matches!(byte, b'0'..=b'9' | b'a'..=b'f'),
            })
}

fn is_plain(value: &Value) -> bool {
    match value {
        Value::Number(number) => number
            .as_i64()
            .map(i64::unsigned_abs)
            .or_else(|| number.as_u64())
            .is_some_and(|magnitude| magnitude <= LARGEST_EXACT_INTEGER),
        Value::Array(items) => items.iter().all(is_plain),
        Value::Object(members) => members
            .iter()
            .all(|(name, member)| name.is_ascii() && is_plain(member)),
        Value::Null | Value::Bool(_) | Value::String(_) => true,
    }
}

fn is_identifier(text: &str) -> bool {
    let mut characters = text.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn is_valid_action(action: &str) -> bool {
    if let Some(unknown) = action.strip_prefix("unknown:") {
        return unknown
            .split_once('.')
            .is_some_and(|(interface_id, method_id)| {
                interface_id.len() == 16
                    && interface_id
                        .bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
                    && method_id
                        .parse::<u16>()
                        .is_ok_and(|parsed| parsed.to_string() == method_id)
            });
    }
    let mut segments = action.split('.');
    segments.clone().count() >= 2 && segments.all(is_identifier)
}

#[cfg(test)]
pub mod fixtures {
    use super::*;

    pub const DEVICE: &str = "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13";
    pub const INSTALLATION: &str = "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70";
    pub const SESSION: &str = "0192f3a4-9e8d-7c6b-8a59-483726150f1e";
    pub const CALL: &str = "0192f3a4-a001-7b2c-9d3e-4f5061728394";
    pub const PID: u64 = 0x9c41_e27a_0b5d_3f86;
    pub const PARAM_HASH: &str = "1f3e5d7c9b0a2f4e6d8c1b3a5f7e9d0c2b4a6f8e1d3c5b7a9f0e2d4c6b8a1f3e";

    pub fn call() -> EntryContent {
        EntryContent {
            kind: Kind::Call,
            device_id: Some(String::from(DEVICE)),
            installation_id: Some(String::from(INSTALLATION)),
            namespace_id: Some(0x5d2e9a1c7b3f8e04),
            epoch: Some(1791278043512408),
            principal: String::from("dawn-0"),
            pid: PID,
            session_id: Some(String::from(SESSION)),
            call_id: Some(String::from(CALL)),
            cap_id: Some(7),
            parent_cap_id: Some(3),
            direction: Some(Direction::ClientToNode),
            action: Some(String::from("ShPortal.sh")),
            interface_id: Some(0xe1c5b0f3a7d29c48),
            method_id: Some(0),
            param_fields: vec![ParamField {
                name: String::from("script"),
                redacted: false,
            }],
            param_cap_ids: vec![8, 9],
            param_hash: String::from(PARAM_HASH),
            result_code: None,
            result_cap_ids: Vec::new(),
            event: None,
            event_detail: None,
        }
    }

    pub fn result() -> EntryContent {
        EntryContent {
            kind: Kind::Result,
            param_fields: Vec::new(),
            param_cap_ids: Vec::new(),
            param_hash: String::new(),
            result_code: Some(ResultCode::Ok),
            result_cap_ids: vec![10],
            ..call()
        }
    }

    pub fn session_open() -> EntryContent {
        let mut detail = Map::new();
        detail.insert(
            String::from("remote_address"),
            Value::from("203.0.113.24:51734"),
        );
        EntryContent {
            device_id: Some(String::from(DEVICE)),
            installation_id: Some(String::from(INSTALLATION)),
            namespace_id: Some(0x5d2e9a1c7b3f8e04),
            epoch: Some(1791278043512408),
            event_detail: Some(detail),
            ..EntryContent::event(Event::SessionOpen, NIGHTFALL_PRINCIPAL)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    #[test]
    fn accepts_well_formed_entries() {
        call().validate().unwrap();
        result().validate().unwrap();
        session_open().validate().unwrap();
        EntryContent {
            result_code: Some(ResultCode::Denied),
            param_hash: String::new(),
            ..call()
        }
        .validate()
        .unwrap();
        EntryContent {
            action: Some(String::from("unknown:9c41e27a0b5d3f86.2")),
            result_code: Some(ResultCode::Denied),
            param_hash: String::new(),
            ..call()
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn refuses_entries_the_schema_refuses() {
        let refused = [
            EntryContent {
                kind: Kind::Checkpoint,
                ..call()
            },
            EntryContent {
                device_id: Some(String::from("3F9C")),
                ..call()
            },
            EntryContent {
                session_id: None,
                ..call()
            },
            EntryContent {
                param_hash: String::new(),
                ..call()
            },
            EntryContent {
                result_code: Some(ResultCode::Ok),
                ..call()
            },
            EntryContent {
                action: Some(String::from("sh")),
                ..call()
            },
            EntryContent {
                action: Some(String::from("unknown:9c41e27a0b5d3f86.2")),
                ..call()
            },
            EntryContent {
                result_code: Some(ResultCode::Denied),
                ..result()
            },
            EntryContent {
                param_hash: String::from(PARAM_HASH),
                ..result()
            },
            EntryContent {
                event: Some(Event::SessionOpen),
                ..call()
            },
            EntryContent {
                session_id: Some(String::from(SESSION)),
                ..session_open()
            },
            EntryContent {
                event: None,
                ..session_open()
            },
            EntryContent::event(Event::ChainLink, NIGHTFALL_PRINCIPAL),
            EntryContent {
                epoch: Some(1),
                ..EntryContent::event(Event::BindingConflict, NIGHTFALL_PRINCIPAL)
            },
            EntryContent::event(Event::SetupCall, "dawn-0"),
            EntryContent::event(Event::QuarantineOverride, "dawn-0"),
        ];
        for entry in refused {
            assert!(entry.validate().is_err(), "accepted {entry:?}");
        }
    }

    #[test]
    fn bounds_the_size_and_the_json_of_an_entry() {
        let largest = EntryContent {
            principal: "\u{1}".repeat(9_800),
            param_fields: vec![
                ParamField {
                    name: String::from("script"),
                    redacted: true,
                };
                40
            ],
            param_cap_ids: vec![LARGEST_EXACT_INTEGER; 40],
            ..call()
        };
        largest.validate().unwrap();
        let entry = LedgerEntry::from_content(
            largest,
            ChainFields {
                id: String::from("01a112fe-d1ba-7684-ae83-5239bd173f3d"),
                time: String::from("2026-10-06T09:15:40.512408217Z"),
                instance: "nightfall-63",
                partition: u32::MAX,
                sequence: LARGEST_EXACT_INTEGER,
                previous_hash: String::from(CHAIN_START_HASH),
            },
        );
        let payload = entry.payload().unwrap();
        assert!(payload.len() > 60_000 && payload.len() <= MAXIMUM_ENTRY_BYTES);

        let many_capabilities = EntryContent {
            result_cap_ids: vec![LARGEST_EXACT_INTEGER; 10_000],
            ..result()
        };
        many_capabilities.validate().unwrap();
        let entry = LedgerEntry::from_content(
            many_capabilities,
            ChainFields {
                id: String::from("01a112fe-d1ba-7684-ae83-5239bd173f3d"),
                time: String::from("2026-10-06T09:15:40.512408217Z"),
                instance: "nightfall-63",
                partition: u32::MAX,
                sequence: LARGEST_EXACT_INTEGER,
                previous_hash: String::from(CHAIN_START_HASH),
            },
        );
        assert!(entry.payload().unwrap().len() <= MAXIMUM_ENTRY_BYTES);

        let refused = [
            EntryContent {
                param_cap_ids: vec![8; 16_000],
                ..call()
            },
            EntryContent {
                principal: "s".repeat(44_000),
                ..call()
            },
            EntryContent {
                epoch: Some(LARGEST_EXACT_INTEGER + 1),
                ..call()
            },
            EntryContent {
                result_cap_ids: vec![u64::MAX],
                ..result()
            },
            EntryContent {
                event_detail: serde_json::from_str(r#"{"latency": 0.5}"#).unwrap(),
                ..session_open()
            },
            EntryContent {
                event_detail: serde_json::from_str(r#"{"nested": [{"count": -9007199254740993}]}"#)
                    .unwrap(),
                ..session_open()
            },
            EntryContent {
                event_detail: serde_json::from_str(r#"{"grün": 1}"#).unwrap(),
                ..session_open()
            },
        ];
        for (position, entry) in refused.iter().enumerate() {
            assert!(entry.validate().is_err(), "accepted entry {position}");
        }
        EntryContent {
            epoch: Some(LARGEST_EXACT_INTEGER),
            ..call()
        }
        .validate()
        .unwrap();
        EntryContent {
            event_detail: serde_json::from_str(
                r#"{"reason": "grün", "counts": [-9007199254740992, 9007199254740992], "set": {"a": null}}"#,
            )
            .unwrap(),
            ..session_open()
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn hashes_the_canonical_form_without_the_hash_member() {
        let mut entry = LedgerEntry::from_content(
            call(),
            ChainFields {
                id: String::from("01a112fe-d1ba-7684-ae83-5239bd173f3d"),
                time: String::from("2026-10-06T09:15:40.512408217Z"),
                instance: "nightfall-0",
                partition: 0,
                sequence: 41,
                previous_hash: String::from(CHAIN_START_HASH),
            },
        );
        entry.seal().unwrap();
        let first = entry.hash.clone();
        entry.hash = String::from("ignored");
        assert_eq!(entry.compute_hash().unwrap(), first);
        entry.sequence = 42;
        assert_ne!(entry.compute_hash().unwrap(), first);
        let parsed: Value = serde_json::from_slice(&entry.payload().unwrap()).unwrap();
        assert_eq!(parsed["namespace_id"], "5d2e9a1c7b3f8e04");
        assert_eq!(parsed["pid"], "11259529557207498630");
        assert_eq!(parsed["interface_id"], "e1c5b0f3a7d29c48");
    }
}
