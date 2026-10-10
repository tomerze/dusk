use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::chain::{Chain, Continuation};
use crate::entry::fixtures::{call, result, session_open};
use crate::entry::{Direction, EntryContent, Event, LedgerEntry, NIGHTFALL_PRINCIPAL, ResultCode};
use crate::signing::fixtures::signer;

fn contracts() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/kafka")
}

fn validator() -> jsonschema::Validator {
    let path = contracts()
        .join("dusk.ledger.schema.json")
        .canonicalize()
        .unwrap();
    let schema: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    jsonschema::options()
        .with_base_uri(format!("file://{}", path.display()))
        .build(&schema)
        .unwrap()
}

fn detail(members: &[(&str, Value)]) -> Option<Map<String, Value>> {
    Some(
        members
            .iter()
            .map(|(name, value)| (String::from(*name), value.clone()))
            .collect(),
    )
}

fn produced() -> Vec<LedgerEntry> {
    let identifier = |number: usize| format!("01a112fe-d1ba-7684-ae83-{number:012x}");
    let time = String::from("2026-10-06T09:15:40.512408217Z");
    let (mut first, _) = Chain::start("nightfall-0", 0, None, Continuation::Start).unwrap();
    let mut entries = Vec::new();
    let contents = [
        session_open(),
        call(),
        result(),
        EntryContent {
            result_code: Some(ResultCode::Denied),
            param_hash: String::new(),
            ..call()
        },
        EntryContent {
            result_code: Some(ResultCode::RateLimited),
            ..call()
        },
        EntryContent {
            action: Some(String::from("unknown:9c41e27a0b5d3f86.2")),
            result_code: Some(ResultCode::Denied),
            param_hash: String::new(),
            ..call()
        },
        EntryContent {
            direction: Some(Direction::NodeToClient),
            pid: 0,
            ..call()
        },
        EntryContent {
            result_code: Some(ResultCode::Revoked),
            ..result()
        },
        EntryContent {
            session_id: None,
            call_id: None,
            cap_id: Some(0),
            parent_cap_id: None,
            direction: Some(Direction::NightfallToNode),
            action: Some(String::from("Dusk.namespaceId")),
            interface_id: Some(0xace6963097d486d6),
            method_id: Some(9),
            ..EntryContent {
                event: Some(Event::SetupCall),
                ..session_open()
            }
        },
        EntryContent {
            namespace_id: None,
            epoch: None,
            event_detail: detail(&[("reason", Value::from("revoked"))]),
            ..EntryContent {
                event: Some(Event::SessionRefused),
                ..session_open()
            }
        },
        EntryContent {
            epoch: None,
            event_detail: None,
            ..EntryContent {
                event: Some(Event::BindingConflict),
                ..session_open()
            }
        },
        EntryContent {
            session_id: call().session_id,
            call_id: call().call_id,
            ..EntryContent {
                event: Some(Event::QuarantineOverride),
                ..session_open()
            }
        },
        EntryContent {
            session_id: call().session_id,
            call_id: call().call_id,
            intent_campaign_id: call().intent_campaign_id,
            intent_principal: call().intent_principal,
            intent_subject: call().intent_subject,
            event_detail: detail(&[
                ("role", Value::from("break-glass")),
                ("rule", Value::from("command_budget")),
            ]),
            ..EntryContent {
                event: Some(Event::AdmissionOverride),
                ..session_open()
            }
        },
        EntryContent {
            result_code: Some(ResultCode::Denied),
            event_detail: detail(&[("rule", Value::from("command_budget"))]),
            ..call()
        },
        EntryContent {
            pid: 2,
            intent_campaign_id: None,
            intent_principal: None,
            intent_subject: None,
            param_hash: String::new(),
            result_code: Some(ResultCode::Denied),
            action: Some(String::from("Dusk.process")),
            event_detail: detail(&[("rule", Value::from("process_without_intent"))]),
            ..call()
        },
        EntryContent {
            session_id: call().session_id,
            event_detail: detail(&[("reason", Value::from("lifecycle_changed"))]),
            ..EntryContent {
                event: Some(Event::MembraneDropped),
                ..session_open()
            }
        },
        EntryContent {
            event_detail: detail(&[("kind", Value::from("provide"))]),
            ..EntryContent {
                event: Some(Event::RpcRejected),
                ..session_open()
            }
        },
        EntryContent {
            event: Some(Event::SessionClose),
            ..session_open()
        },
    ];
    for (number, content) in contents.into_iter().enumerate() {
        content.validate().unwrap();
        entries.push(
            first
                .append(identifier(number), time.clone(), content)
                .unwrap(),
        );
    }
    entries.push(
        first
            .checkpoint(&signer(2), identifier(100), time.clone())
            .unwrap()
            .unwrap(),
    );
    let tail = entries.last().unwrap().payload().unwrap();
    let (mut resumed, event) =
        Chain::start("nightfall-0", 0, Some(&tail), Continuation::Resume).unwrap();
    entries.push(
        resumed
            .append(identifier(101), time.clone(), event.unwrap())
            .unwrap(),
    );
    let tail = entries.last().unwrap().payload().unwrap();
    let (mut linked, event) =
        Chain::start("nightfall-1", 0, Some(&tail), Continuation::Start).unwrap();
    entries.push(
        linked
            .append(identifier(102), time, event.unwrap())
            .unwrap(),
    );
    assert_eq!(entries.last().unwrap().principal, NIGHTFALL_PRINCIPAL);
    entries
}

#[test]
fn every_entry_the_ledger_writes_matches_the_contract() {
    let validator = validator();
    for entry in produced() {
        let value: Value = serde_json::from_slice(&entry.payload().unwrap()).unwrap();
        let errors: Vec<String> = validator
            .iter_errors(&value)
            .map(|error| error.to_string())
            .collect();
        assert!(
            errors.is_empty(),
            "{} {:?}: {errors:?}",
            entry.sequence,
            entry.event
        );
    }
}

#[test]
fn every_valid_contract_example_reads_as_a_ledger_entry() {
    let validator = validator();
    let mut read = 0;
    for file in std::fs::read_dir(contracts().join("examples/dusk.ledger")).unwrap() {
        let path = file.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if name.starts_with("invalid-") {
            assert!(!validator.is_valid(&value), "{name} validates");
            continue;
        }
        assert!(validator.is_valid(&value), "{name} does not validate");
        let entry: LedgerEntry =
            serde_json::from_value(value).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(entry.schema, crate::entry::SCHEMA);
        read += 1;
    }
    assert!(read >= 20, "only {read} valid examples");
}
