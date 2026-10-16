use crate::directory::{NodeIdentity, parse_identifier};
use nightfall_provisioning::state::{Lifecycle, NodeStateView};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::RwLock;

pub const SCHEMA: &str = "dusk.node-state/v1";
pub const SCHEMA_V2: &str = "dusk.node-state/v2";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    Device(u128),
    Installation(u128, u128),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateRecord {
    pub scope: Scope,
    pub lifecycle: Option<Lifecycle>,
    pub time_nanoseconds: i64,
}

fn nanoseconds(text: &str) -> Option<i64> {
    time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
        .ok()
        .and_then(|moment| i64::try_from(moment.unix_timestamp_nanos()).ok())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    State(StateRecord),
    Complete,
}

const COMPLETE_PREFIX: &[u8] = b"complete/";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompleteMessage {
    schema: String,
    id: String,
    time: String,
    record: String,
    actor: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StateMessage {
    schema: String,
    id: String,
    time: String,
    scope: String,
    device_id: String,
    installation_id: Option<String>,
    lifecycle: String,
    #[serde(rename = "reason")]
    _reason: Option<String>,
    actor: String,
}

pub fn lifecycle_named(name: &str) -> Option<Lifecycle> {
    Some(match name {
        "enrolled" => Lifecycle::Enrolled,
        "active" => Lifecycle::Active,
        "quarantined" => Lifecycle::Quarantined,
        "retired" => Lifecycle::Retired,
        "revoked" => Lifecycle::Revoked,
        _ => return None,
    })
}

fn severity(lifecycle: Option<Lifecycle>) -> u8 {
    match lifecycle {
        Some(Lifecycle::Revoked) => 4,
        Some(Lifecycle::Retired) => 3,
        Some(Lifecycle::Quarantined) => 2,
        Some(Lifecycle::Active) => 1,
        Some(Lifecycle::Enrolled) | None => 0,
    }
}

pub fn parse_key(key: &[u8]) -> Result<Scope, String> {
    let key = std::str::from_utf8(key).map_err(|_| "a key that is not UTF-8".to_string())?;
    let parts: Vec<&str> = key.split('/').collect();
    let identifier = |text: &str| parse_identifier(text).ok_or_else(|| format!("key {key:?}"));
    match parts.as_slice() {
        ["device", device] => Ok(Scope::Device(identifier(device)?)),
        ["installation", device, installation] => Ok(Scope::Installation(
            identifier(device)?,
            identifier(installation)?,
        )),
        _ => Err(format!("key {key:?}")),
    }
}

fn parse_complete(key: &[u8], payload: Option<&[u8]>) -> Result<Entry, String> {
    let index = &key[COMPLETE_PREFIX.len()..];
    if index.is_empty() || !index.iter().all(u8::is_ascii_digit) {
        return Err(format!("key {:?}", String::from_utf8_lossy(key)));
    }
    let payload = payload.ok_or("a tombstone for a completion record")?;
    let message: CompleteMessage =
        serde_json::from_slice(payload).map_err(|error| error.to_string())?;
    if message.schema != SCHEMA_V2 {
        return Err(format!("schema {:?}", message.schema));
    }
    if uuid::Uuid::parse_str(&message.id).is_err()
        || crate::events::parse_time_milliseconds(&message.time).is_none()
        || message.record != "complete"
        || message.actor.is_empty()
    {
        return Err("an invalid id, time, record or actor".to_string());
    }
    Ok(Entry::Complete)
}

pub fn parse(
    key: Option<&[u8]>,
    payload: Option<&[u8]>,
    timestamp_milliseconds: i64,
) -> Result<Entry, String> {
    let key = key.ok_or("a record without a key")?;
    if key.starts_with(COMPLETE_PREFIX) {
        return parse_complete(key, payload);
    }
    let scope = parse_key(key)?;
    let Some(payload) = payload else {
        return Ok(Entry::State(StateRecord {
            scope,
            lifecycle: None,
            time_nanoseconds: timestamp_milliseconds.saturating_mul(1_000_000),
        }));
    };
    let message: StateMessage =
        serde_json::from_slice(payload).map_err(|error| error.to_string())?;
    if message.schema != SCHEMA && message.schema != SCHEMA_V2 {
        return Err(format!("schema {:?}", message.schema));
    }
    let time_nanoseconds = nanoseconds(&message.time).ok_or("an invalid time")?;
    if uuid::Uuid::parse_str(&message.id).is_err() || message.actor.is_empty() {
        return Err("an invalid id or actor".to_string());
    }
    let device = parse_identifier(&message.device_id).ok_or("an invalid device id")?;
    let payload_scope = match (message.scope.as_str(), message.installation_id.as_deref()) {
        ("device", None) => Scope::Device(device),
        ("installation", Some(installation)) => Scope::Installation(
            device,
            parse_identifier(installation).ok_or("an invalid installation id")?,
        ),
        (scope, installation) => {
            return Err(format!(
                "scope {scope:?} with installation {installation:?}"
            ));
        }
    };
    if payload_scope != scope {
        return Err("the key names another node than the value".to_string());
    }
    let lifecycle = lifecycle_named(&message.lifecycle)
        .ok_or_else(|| format!("lifecycle {:?}", message.lifecycle))?;
    Ok(Entry::State(StateRecord {
        scope,
        lifecycle: Some(lifecycle),
        time_nanoseconds,
    }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Blocked,
    QuarantineChanged,
    Unchanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flip {
    pub scope: Scope,
    pub change: Change,
}

impl Scope {
    pub fn matches(&self, identity: NodeIdentity) -> bool {
        match *self {
            Scope::Device(device) => device == identity.device,
            Scope::Installation(device, installation) => {
                device == identity.device && installation == identity.installation
            }
        }
    }
}

impl Flip {
    pub fn matches(&self, identity: NodeIdentity) -> bool {
        self.scope.matches(identity)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Held {
    lifecycle: Option<Lifecycle>,
    time_nanoseconds: i64,
    partition: i32,
}

#[derive(Default)]
pub struct NodeStates {
    states: RwLock<HashMap<Scope, Held>>,
}

impl NodeStates {
    pub fn new() -> NodeStates {
        NodeStates::default()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<Scope, Held>> {
        self.states
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lifecycle(states: &HashMap<Scope, Held>, scope: Scope) -> Option<Lifecycle> {
        states.get(&scope).and_then(|held| held.lifecycle)
    }

    pub fn effective(&self, identity: NodeIdentity) -> Option<Lifecycle> {
        let states = self.read();
        let device = NodeStates::lifecycle(&states, Scope::Device(identity.device));
        let installation = NodeStates::lifecycle(
            &states,
            Scope::Installation(identity.device, identity.installation),
        );
        if severity(device) >= severity(installation) {
            device
        } else {
            installation
        }
    }

    pub fn len(&self) -> usize {
        self.read()
            .values()
            .filter(|held| held.lifecycle.is_some())
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn apply(&self, record: StateRecord, partition: i32) -> Flip {
        let mut states = self
            .states
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if states.get(&record.scope).is_some_and(|held| {
            held.partition != partition && record.time_nanoseconds < held.time_nanoseconds
        }) {
            return Flip {
                scope: record.scope,
                change: Change::Unchanged,
            };
        }
        let held = match record.lifecycle {
            Some(Lifecycle::Enrolled) | Some(Lifecycle::Active) | None => None,
            lifecycle => lifecycle,
        };
        let previous = states
            .insert(
                record.scope,
                Held {
                    lifecycle: held,
                    time_nanoseconds: record.time_nanoseconds,
                    partition,
                },
            )
            .and_then(|held| held.lifecycle);
        let blocked =
            |lifecycle: Option<Lifecycle>| lifecycle.is_some_and(Lifecycle::blocks_certificates);
        let quarantined = |lifecycle: Option<Lifecycle>| lifecycle == Some(Lifecycle::Quarantined);
        let change = if blocked(record.lifecycle) && !blocked(previous) {
            Change::Blocked
        } else if quarantined(record.lifecycle) != quarantined(previous) {
            Change::QuarantineChanged
        } else {
            Change::Unchanged
        };
        Flip {
            scope: record.scope,
            change,
        }
    }
}

impl NodeStateView for NodeStates {
    fn device(&self, device_id: &str) -> Option<Lifecycle> {
        let device = parse_identifier(device_id)?;
        NodeStates::lifecycle(&self.read(), Scope::Device(device))
    }

    fn installation(&self, device_id: &str, installation_id: &str) -> Option<Lifecycle> {
        let device = parse_identifier(device_id)?;
        let installation = parse_identifier(installation_id)?;
        NodeStates::lifecycle(&self.read(), Scope::Installation(device, installation))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::tests::{assert_valid, validator};
    use serde_json::json;

    const DEVICE: &str = "00112233445566778899aabbccddeeff";
    const INSTALLATION: &str = "ffeeddccbbaa99887766554433221100";

    pub fn message(scope: &str, installation: Option<&str>, lifecycle: &str) -> serde_json::Value {
        json!({
            "schema": SCHEMA,
            "id": crate::events::message_id(),
            "time": crate::events::now(),
            "scope": scope,
            "device_id": DEVICE,
            "installation_id": installation,
            "lifecycle": lifecycle,
            "reason": "test",
            "actor": "twilight",
        })
    }

    fn identity() -> NodeIdentity {
        NodeIdentity::parse(DEVICE, INSTALLATION).unwrap()
    }

    fn record(key: &str, payload: Option<serde_json::Value>) -> StateRecord {
        let bytes = payload.map(|payload| payload.to_string().into_bytes());
        match parse(
            Some(key.as_bytes()),
            bytes.as_deref(),
            crate::kafka::unix_milliseconds(),
        )
        .unwrap()
        {
            Entry::State(state) => state,
            Entry::Complete => panic!("{key} parsed as a completion record"),
        }
    }

    fn completion() -> serde_json::Value {
        json!({
            "schema": SCHEMA_V2,
            "id": crate::events::message_id(),
            "time": crate::events::now(),
            "record": "complete",
            "actor": "twilight-0",
        })
    }

    fn parse_value(key: &str, payload: Option<&serde_json::Value>) -> Result<Entry, String> {
        let bytes = payload.map(|payload| payload.to_string().into_bytes());
        parse(Some(key.as_bytes()), bytes.as_deref(), 0)
    }

    #[test]
    fn reads_a_completion_record_under_a_numbered_complete_key() {
        let validator = validator("dusk.node-state.v2");
        let complete = completion();
        assert_valid(&validator, &complete);
        assert_eq!(
            parse_value("complete/0", Some(&complete)),
            Ok(Entry::Complete)
        );
        assert_eq!(
            parse_value("complete/17", Some(&complete)),
            Ok(Entry::Complete)
        );
        assert!(parse_value("complete/", Some(&complete)).is_err());
        assert!(parse_value("complete/x", Some(&complete)).is_err());
        assert!(parse_value("complete/0", None).is_err());
        let mut wrong = complete.clone();
        wrong["record"] = json!("done");
        assert!(parse_value("complete/0", Some(&wrong)).is_err());
        let mut extra = complete.clone();
        extra["device_id"] = json!(DEVICE);
        assert!(parse_value("complete/0", Some(&extra)).is_err());
        let mut anonymous = complete;
        anonymous["actor"] = json!("");
        assert!(parse_value("complete/0", Some(&anonymous)).is_err());
        let state = message("device", None, "revoked");
        assert!(parse_value("complete/0", Some(&state)).is_err());
        let mut first_version = completion();
        first_version["schema"] = json!(SCHEMA);
        assert!(parse_value("complete/0", Some(&first_version)).is_err());
        let mut later_state = message("installation", Some(INSTALLATION), "revoked");
        later_state["schema"] = json!(SCHEMA_V2);
        assert_valid(&validator, &later_state);
        assert!(matches!(
            parse_value(
                &format!("installation/{DEVICE}/{INSTALLATION}"),
                Some(&later_state)
            ),
            Ok(Entry::State(_))
        ));
    }

    #[test]
    fn reads_records_and_tombstones() {
        let validator = validator("dusk.node-state");
        let installation = message("installation", Some(INSTALLATION), "revoked");
        assert_valid(&validator, &installation);
        let key = format!("installation/{DEVICE}/{INSTALLATION}");
        let parsed = record(&key, Some(installation));
        assert_eq!(
            parsed.scope,
            Scope::Installation(identity().device, identity().installation)
        );
        assert_eq!(parsed.lifecycle, Some(Lifecycle::Revoked));
        assert_eq!(record(&format!("device/{DEVICE}"), None).lifecycle, None);
        let mismatched = message("device", None, "revoked");
        assert!(
            parse(
                Some(key.as_bytes()),
                Some(mismatched.to_string().as_bytes()),
                0
            )
            .is_err()
        );
        assert!(parse(Some(b"device/xyz"), None, 0).is_err());
        assert!(parse(None, None, 0).is_err());
        let mut unknown = message("device", None, "lost");
        assert!(
            parse(
                Some(format!("device/{DEVICE}").as_bytes()),
                Some(unknown.to_string().as_bytes()),
                0
            )
            .is_err()
        );
        unknown["lifecycle"] = json!("quarantined");
        unknown["extra"] = json!(true);
        assert!(
            parse(
                Some(format!("device/{DEVICE}").as_bytes()),
                Some(unknown.to_string().as_bytes()),
                0
            )
            .is_err()
        );
    }

    #[test]
    fn a_device_scope_block_covers_every_installation() {
        let states = NodeStates::new();
        let device_key = format!("device/{DEVICE}");
        let flip = states.apply(
            record(&device_key, Some(message("device", None, "retired"))),
            0,
        );
        assert_eq!(flip.change, Change::Blocked);
        assert!(flip.matches(identity()));
        assert_eq!(states.effective(identity()), Some(Lifecycle::Retired));
        assert_eq!(states.device(DEVICE), Some(Lifecycle::Retired));
        assert_eq!(states.installation(DEVICE, INSTALLATION), None);
        let flip = states.apply(record(&device_key, None), 0);
        assert_eq!(flip.change, Change::Unchanged);
        assert!(states.is_empty());
    }

    #[test]
    fn reports_quarantine_flips_both_ways_and_keeps_only_non_default_states() {
        let states = NodeStates::new();
        let key = format!("installation/{DEVICE}/{INSTALLATION}");
        let into = states.apply(
            record(
                &key,
                Some(message("installation", Some(INSTALLATION), "quarantined")),
            ),
            0,
        );
        assert_eq!(into.change, Change::QuarantineChanged);
        assert_eq!(states.effective(identity()), Some(Lifecycle::Quarantined));
        let out = states.apply(
            record(
                &key,
                Some(message("installation", Some(INSTALLATION), "active")),
            ),
            0,
        );
        assert_eq!(out.change, Change::QuarantineChanged);
        assert_eq!(states.len(), 0);
        let blocked = states.apply(
            record(
                &key,
                Some(message("installation", Some(INSTALLATION), "revoked")),
            ),
            0,
        );
        assert_eq!(blocked.change, Change::Blocked);
        let again = states.apply(
            record(
                &key,
                Some(message("installation", Some(INSTALLATION), "revoked")),
            ),
            0,
        );
        assert_eq!(again.change, Change::Unchanged);
        let other = NodeIdentity::parse(DEVICE, "0123456789abcdef0123456789abcdef").unwrap();
        assert!(!blocked.matches(other));
        assert_eq!(states.effective(other), None);
    }

    #[test]
    fn an_older_record_read_after_a_newer_one_is_ignored() {
        let states = NodeStates::new();
        let installation = |lifecycle, time_nanoseconds| StateRecord {
            scope: Scope::Installation(identity().device, identity().installation),
            lifecycle,
            time_nanoseconds,
        };
        let device = |lifecycle, time_nanoseconds| StateRecord {
            scope: Scope::Device(identity().device),
            lifecycle,
            time_nanoseconds,
        };
        let revoked = states.apply(installation(Some(Lifecycle::Revoked), 2_000), 1);
        assert_eq!(revoked.change, Change::Blocked);
        let older = states.apply(installation(Some(Lifecycle::Active), 1_000), 0);
        assert_eq!(older.change, Change::Unchanged);
        assert_eq!(states.effective(identity()), Some(Lifecycle::Revoked));
        states.apply(device(None, 5_000), 2);
        let retired = states.apply(device(Some(Lifecycle::Retired), 4_000), 0);
        assert_eq!(retired.change, Change::Unchanged);
        assert_eq!(states.device(DEVICE), None);
        assert_eq!(states.effective(identity()), Some(Lifecycle::Revoked));
    }

    #[test]
    fn a_later_record_on_the_same_partition_applies_whatever_its_time() {
        let states = NodeStates::new();
        let installation = |lifecycle, time_nanoseconds| StateRecord {
            scope: Scope::Installation(identity().device, identity().installation),
            lifecycle,
            time_nanoseconds,
        };
        states.apply(installation(Some(Lifecycle::Quarantined), 2_000), 1);
        let released = states.apply(installation(Some(Lifecycle::Active), 1_000), 1);
        assert_eq!(released.change, Change::QuarantineChanged);
        assert_eq!(states.effective(identity()), None);
        assert!(states.is_empty());
    }

    #[test]
    fn the_worse_of_device_and_installation_state_wins() {
        let states = NodeStates::new();
        states.apply(
            record(
                &format!("device/{DEVICE}"),
                Some(message("device", None, "quarantined")),
            ),
            0,
        );
        states.apply(
            record(
                &format!("installation/{DEVICE}/{INSTALLATION}"),
                Some(message("installation", Some(INSTALLATION), "revoked")),
            ),
            0,
        );
        assert_eq!(states.effective(identity()), Some(Lifecycle::Revoked));
    }
}
