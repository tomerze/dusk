use crate::directory::{NodeIdentity, parse_identifier};
use nightfall_provisioning::state::{Lifecycle, NodeStateView};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::RwLock;

pub const SCHEMA: &str = "dusk.node-state/v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    Device(u128),
    Installation(u128, u128),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateRecord {
    pub scope: Scope,
    pub lifecycle: Option<Lifecycle>,
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

pub fn parse(key: Option<&[u8]>, payload: Option<&[u8]>) -> Result<StateRecord, String> {
    let scope = parse_key(key.ok_or("a record without a key")?)?;
    let Some(payload) = payload else {
        return Ok(StateRecord {
            scope,
            lifecycle: None,
        });
    };
    let message: StateMessage =
        serde_json::from_slice(payload).map_err(|error| error.to_string())?;
    if message.schema != SCHEMA {
        return Err(format!("schema {:?}", message.schema));
    }
    if uuid::Uuid::parse_str(&message.id).is_err()
        || crate::events::parse_time_milliseconds(&message.time).is_none()
        || message.actor.is_empty()
    {
        return Err("an invalid id, time or actor".to_string());
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
    Ok(StateRecord {
        scope,
        lifecycle: Some(lifecycle),
    })
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

#[derive(Default)]
pub struct NodeStates {
    states: RwLock<HashMap<Scope, Lifecycle>>,
}

impl NodeStates {
    pub fn new() -> NodeStates {
        NodeStates::default()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<Scope, Lifecycle>> {
        self.states
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn effective(&self, identity: NodeIdentity) -> Option<Lifecycle> {
        let states = self.read();
        let device = states.get(&Scope::Device(identity.device)).copied();
        let installation = states
            .get(&Scope::Installation(identity.device, identity.installation))
            .copied();
        if severity(device) >= severity(installation) {
            device
        } else {
            installation
        }
    }

    pub fn len(&self) -> usize {
        self.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.read().is_empty()
    }

    pub fn apply(&self, record: StateRecord) -> Flip {
        let mut states = self
            .states
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = match record.lifecycle {
            Some(Lifecycle::Enrolled) | Some(Lifecycle::Active) | None => {
                states.remove(&record.scope)
            }
            Some(lifecycle) => states.insert(record.scope, lifecycle),
        };
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
        self.read().get(&Scope::Device(device)).copied()
    }

    fn installation(&self, device_id: &str, installation_id: &str) -> Option<Lifecycle> {
        let device = parse_identifier(device_id)?;
        let installation = parse_identifier(installation_id)?;
        self.read()
            .get(&Scope::Installation(device, installation))
            .copied()
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
        parse(Some(key.as_bytes()), bytes.as_deref()).unwrap()
    }

    #[test]
    fn reads_records_and_tombstones() {
        let validator = validator("dusk.node-state");
        let installation = message("installation", Some(INSTALLATION), "revoked");
        assert_valid(&validator, &installation);
        let key = format!("installation/{DEVICE}/{INSTALLATION}");
        assert_eq!(
            record(&key, Some(installation)),
            StateRecord {
                scope: Scope::Installation(identity().device, identity().installation),
                lifecycle: Some(Lifecycle::Revoked)
            }
        );
        assert_eq!(record(&format!("device/{DEVICE}"), None).lifecycle, None);
        let mismatched = message("device", None, "revoked");
        assert!(
            parse(
                Some(key.as_bytes()),
                Some(mismatched.to_string().as_bytes())
            )
            .is_err()
        );
        assert!(parse(Some(b"device/xyz"), None).is_err());
        assert!(parse(None, None).is_err());
        let mut unknown = message("device", None, "lost");
        assert!(
            parse(
                Some(format!("device/{DEVICE}").as_bytes()),
                Some(unknown.to_string().as_bytes())
            )
            .is_err()
        );
        unknown["lifecycle"] = json!("quarantined");
        unknown["extra"] = json!(true);
        assert!(
            parse(
                Some(format!("device/{DEVICE}").as_bytes()),
                Some(unknown.to_string().as_bytes())
            )
            .is_err()
        );
    }

    #[test]
    fn a_device_scope_block_covers_every_installation() {
        let states = NodeStates::new();
        let device_key = format!("device/{DEVICE}");
        let flip = states.apply(record(
            &device_key,
            Some(message("device", None, "retired")),
        ));
        assert_eq!(flip.change, Change::Blocked);
        assert!(flip.matches(identity()));
        assert_eq!(states.effective(identity()), Some(Lifecycle::Retired));
        assert_eq!(states.device(DEVICE), Some(Lifecycle::Retired));
        assert_eq!(states.installation(DEVICE, INSTALLATION), None);
        let flip = states.apply(record(&device_key, None));
        assert_eq!(flip.change, Change::Unchanged);
        assert!(states.is_empty());
    }

    #[test]
    fn reports_quarantine_flips_both_ways_and_keeps_only_non_default_states() {
        let states = NodeStates::new();
        let key = format!("installation/{DEVICE}/{INSTALLATION}");
        let into = states.apply(record(
            &key,
            Some(message("installation", Some(INSTALLATION), "quarantined")),
        ));
        assert_eq!(into.change, Change::QuarantineChanged);
        assert_eq!(states.effective(identity()), Some(Lifecycle::Quarantined));
        let out = states.apply(record(
            &key,
            Some(message("installation", Some(INSTALLATION), "active")),
        ));
        assert_eq!(out.change, Change::QuarantineChanged);
        assert_eq!(states.len(), 0);
        let blocked = states.apply(record(
            &key,
            Some(message("installation", Some(INSTALLATION), "revoked")),
        ));
        assert_eq!(blocked.change, Change::Blocked);
        let again = states.apply(record(
            &key,
            Some(message("installation", Some(INSTALLATION), "revoked")),
        ));
        assert_eq!(again.change, Change::Unchanged);
        let other = NodeIdentity::parse(DEVICE, "0123456789abcdef0123456789abcdef").unwrap();
        assert!(!blocked.matches(other));
        assert_eq!(states.effective(other), None);
    }

    #[test]
    fn the_worse_of_device_and_installation_state_wins() {
        let states = NodeStates::new();
        states.apply(record(
            &format!("device/{DEVICE}"),
            Some(message("device", None, "quarantined")),
        ));
        states.apply(record(
            &format!("installation/{DEVICE}/{INSTALLATION}"),
            Some(message("installation", Some(INSTALLATION), "revoked")),
        ));
        assert_eq!(states.effective(identity()), Some(Lifecycle::Revoked));
    }
}
