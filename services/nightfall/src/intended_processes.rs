use crate::events::parse_time_milliseconds;
use nightfall_membrane::admission::{IntendedProcess, Intent, NO_PROCESS, NodeKey};
use serde::Deserialize;
use std::sync::Arc;

pub const SCHEMA: &str = "dusk.intended-processes/v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntendedRecord {
    pub node: NodeKey,
    pub pid: u64,
    pub process: Option<IntendedProcess>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IntendedMessage {
    schema: String,
    #[serde(rename = "id")]
    _id: String,
    #[serde(rename = "time")]
    _time: String,
    pid: String,
    device_id: String,
    installation_id: String,
    campaign_id: Option<String>,
    #[serde(rename = "action_kind")]
    _action_kind: String,
    principal: String,
    subject: String,
    created_at: String,
    expires_at: String,
    max_commands: u32,
    default_shell_commands: u32,
}

fn parse_pid(text: &str) -> Option<u64> {
    let canonical = !text.is_empty()
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    canonical.then(|| text.parse().ok()).flatten()
}

pub fn parse_key(key: &[u8]) -> Result<(NodeKey, u64), String> {
    let key = std::str::from_utf8(key).map_err(|_| "a key that is not UTF-8".to_string())?;
    let parts: Vec<&str> = key.split('/').collect();
    let [device, installation, pid] = parts.as_slice() else {
        return Err(format!("key {key:?}"));
    };
    let node = NodeKey::parse(device, installation).ok_or_else(|| format!("key {key:?}"))?;
    let pid = parse_pid(pid)
        .filter(|pid| *pid != NO_PROCESS)
        .ok_or_else(|| format!("key {key:?}"))?;
    Ok((node, pid))
}

pub fn parse(key: Option<&[u8]>, payload: Option<&[u8]>) -> Result<IntendedRecord, String> {
    let (node, pid) = parse_key(key.ok_or("a record without a key")?)?;
    let Some(payload) = payload else {
        return Ok(IntendedRecord {
            node,
            pid,
            process: None,
        });
    };
    let message: IntendedMessage =
        serde_json::from_slice(payload).map_err(|error| error.to_string())?;
    if message.schema != SCHEMA {
        return Err(format!("schema {:?}", message.schema));
    }
    if NodeKey::parse(&message.device_id, &message.installation_id) != Some(node)
        || parse_pid(&message.pid) != Some(pid)
    {
        return Err("the key names another process than the value".to_string());
    }
    let (Some(created_at_ms), Some(expires_at_ms)) = (
        parse_time_milliseconds(&message.created_at),
        parse_time_milliseconds(&message.expires_at),
    ) else {
        return Err("an invalid created_at or expires_at".to_string());
    };
    if message.principal.is_empty() || message.subject.is_empty() {
        return Err("an empty principal or subject".to_string());
    }
    Ok(IntendedRecord {
        node,
        pid,
        process: Some(IntendedProcess {
            created_at_ms,
            expires_at_ms,
            max_commands: message.max_commands,
            default_shell_commands: message.default_shell_commands,
            intent: Arc::new(Intent {
                campaign_id: message.campaign_id,
                principal: message.principal,
                subject: message.subject,
            }),
        }),
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::events::tests::{assert_valid, validator};
    use serde_json::json;

    pub const DEVICE: &str = "00112233445566778899aabbccddeeff";
    pub const INSTALLATION: &str = "ffeeddccbbaa99887766554433221100";
    pub const CAMPAIGN: &str = "0192f3a4-5b6c-7d8e-9f01-23456789abcd";

    pub fn message(pid: u64, expires_at: &str) -> serde_json::Value {
        json!({
            "schema": SCHEMA,
            "id": crate::events::message_id(),
            "time": crate::events::now(),
            "pid": pid.to_string(),
            "device_id": DEVICE,
            "installation_id": INSTALLATION,
            "campaign_id": CAMPAIGN,
            "action_kind": "run_script",
            "principal": "token:0192f3a4-1111-7d8e-9f01-23456789abcd",
            "subject": format!("campaign:{CAMPAIGN}"),
            "created_at": "2026-10-06T09:15:39.511902330Z",
            "expires_at": expires_at,
            "max_commands": 2,
            "default_shell_commands": 3,
        })
    }

    pub fn key(pid: u64) -> String {
        format!("{DEVICE}/{INSTALLATION}/{pid}")
    }

    #[test]
    fn reads_records_and_tombstones() {
        let validator = validator("dusk.intended-processes");
        let payload = message(42, "2026-10-06T09:21:39.511902330Z");
        assert_valid(&validator, &payload);
        let record = parse(
            Some(key(42).as_bytes()),
            Some(payload.to_string().as_bytes()),
        )
        .unwrap();
        let node = NodeKey::parse(DEVICE, INSTALLATION).unwrap();
        assert_eq!(record.node, node);
        assert_eq!(record.pid, 42);
        let process = record.process.unwrap();
        assert_eq!(process.max_commands, 2);
        assert_eq!(process.default_shell_commands, 3);
        assert_eq!(process.expires_at_ms - process.created_at_ms, 360_000);
        assert_eq!(process.intent.campaign_id.as_deref(), Some(CAMPAIGN));
        assert_eq!(process.intent.subject, format!("campaign:{CAMPAIGN}"));
        assert_eq!(
            parse(Some(key(42).as_bytes()), None).unwrap(),
            IntendedRecord {
                node,
                pid: 42,
                process: None
            }
        );
    }

    #[test]
    fn refuses_records_whose_key_and_value_disagree_or_are_malformed() {
        let payload = message(42, "2026-10-06T09:21:39.511902330Z").to_string();
        for bad_key in [
            key(43),
            key(0),
            format!("{DEVICE}/{INSTALLATION}/042"),
            format!("{DEVICE}/{INSTALLATION}"),
            format!("{DEVICE}/00000000000000000000000000000001/42"),
            "device/x/y".to_string(),
        ] {
            assert!(
                parse(Some(bad_key.as_bytes()), Some(payload.as_bytes())).is_err(),
                "{bad_key}"
            );
        }
        assert!(parse(None, None).is_err());
        let mut unknown = message(42, "2026-10-06T09:21:39.511902330Z");
        unknown["attempt"] = json!(1);
        assert!(
            parse(
                Some(key(42).as_bytes()),
                Some(unknown.to_string().as_bytes())
            )
            .is_err()
        );
        let mut late = message(42, "not a time");
        late["schema"] = json!(SCHEMA);
        assert!(parse(Some(key(42).as_bytes()), Some(late.to_string().as_bytes())).is_err());
    }
}
