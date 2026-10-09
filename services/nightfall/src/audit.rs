use capnp::capability::Promise;
use nightfall_ledger::entry::{
    Direction as LedgerDirection, EntryContent, Event, Kind, NIGHTFALL_PRINCIPAL, ParamField,
    ResultCode as LedgerResultCode,
};
use nightfall_ledger::writer::{CommitFailure, LedgerWriter, Refused, Reservation};
use nightfall_membrane::audit::{
    AuditEntry, AuditEvent, AuditRefused, AuditReservation, AuditSink, AuditSlot, Direction,
    EntryKind, ResultCode,
};
use serde_json::{Map, Value};

fn kind(kind: EntryKind) -> Kind {
    match kind {
        EntryKind::Call => Kind::Call,
        EntryKind::Result => Kind::Result,
        EntryKind::Event => Kind::Event,
    }
}

fn direction(direction: Direction) -> LedgerDirection {
    match direction {
        Direction::ClientToNode => LedgerDirection::ClientToNode,
        Direction::NodeToClient => LedgerDirection::NodeToClient,
        Direction::NightfallToNode => LedgerDirection::NightfallToNode,
    }
}

fn result_code(code: ResultCode) -> LedgerResultCode {
    match code {
        ResultCode::Ok => LedgerResultCode::Ok,
        ResultCode::Failed => LedgerResultCode::Failed,
        ResultCode::Overloaded => LedgerResultCode::Overloaded,
        ResultCode::Disconnected => LedgerResultCode::Disconnected,
        ResultCode::Unimplemented => LedgerResultCode::Unimplemented,
        ResultCode::Denied => LedgerResultCode::Denied,
        ResultCode::RateLimited => LedgerResultCode::RateLimited,
        ResultCode::Revoked => LedgerResultCode::Revoked,
    }
}

fn event(event: AuditEvent) -> Event {
    match event {
        AuditEvent::SessionOpen => Event::SessionOpen,
        AuditEvent::SessionClose => Event::SessionClose,
        AuditEvent::SessionRefused => Event::SessionRefused,
        AuditEvent::SetupCall => Event::SetupCall,
        AuditEvent::MembraneDropped => Event::MembraneDropped,
        AuditEvent::BindingConflict => Event::BindingConflict,
        AuditEvent::RpcRejected => Event::RpcRejected,
        AuditEvent::ChainLink => Event::ChainLink,
        AuditEvent::ChainResumed => Event::ChainResumed,
        AuditEvent::QuarantineOverride => Event::QuarantineOverride,
    }
}

pub fn content(entry: AuditEntry) -> EntryContent {
    EntryContent {
        kind: kind(entry.kind),
        device_id: entry.device_id,
        installation_id: entry.installation_id,
        namespace_id: entry.namespace_id,
        epoch: entry.epoch,
        principal: entry.principal,
        pid: entry.pid,
        session_id: entry.session_id,
        call_id: entry.call_id,
        cap_id: entry.cap_id,
        parent_cap_id: entry.parent_cap_id,
        direction: entry.direction.map(direction),
        action: entry.action,
        interface_id: entry.interface_id,
        method_id: entry.method_id,
        param_fields: entry
            .param_fields
            .into_iter()
            .map(|field| ParamField {
                name: field.name,
                redacted: field.redacted,
            })
            .collect(),
        param_cap_ids: entry.param_cap_ids,
        param_hash: entry.param_hash,
        result_code: entry.result_code.map(result_code),
        result_cap_ids: entry.result_cap_ids,
        event: entry.event.map(event),
        event_detail: entry.event_detail,
    }
}

fn commit_error(failure: CommitFailure) -> capnp::Error {
    match failure {
        CommitFailure::TimedOut => {
            capnp::Error::overloaded("the ledger did not commit the call in time".to_string())
        }
        CommitFailure::Unavailable | CommitFailure::Fenced => {
            capnp::Error::failed("unavailable: ledger".to_string())
        }
    }
}

struct LedgerSlot {
    reservation: Reservation,
}

impl AuditSlot for LedgerSlot {
    fn record(mut self: Box<Self>, entry: AuditEntry) -> Option<Promise<(), capnp::Error>> {
        let write_ahead = entry.write_ahead;
        let action = entry.action.clone();
        let content = content(entry);
        if write_ahead {
            return Some(match self.reservation.record_write_ahead(content) {
                Ok(commit) => {
                    Promise::from_future(async move { commit.await.map_err(commit_error) })
                }
                Err(refused) => {
                    tracing::error!(?action, %refused, "the ledger refused a write-ahead call entry");
                    Promise::err(capnp::Error::failed("unavailable: ledger".to_string()))
                }
            });
        }
        if let Err(refused) = self.reservation.record(content) {
            tracing::error!(?action, %refused, "the ledger refused an entry; it is not recorded");
        }
        None
    }
}

fn refusal(refused: Refused) -> AuditRefused {
    AuditRefused {
        reason: refused.to_string(),
    }
}

#[derive(Clone)]
pub struct LedgerAudit {
    writer: LedgerWriter,
}

impl LedgerAudit {
    pub fn new(writer: LedgerWriter) -> LedgerAudit {
        LedgerAudit { writer }
    }

    pub fn writer(&self) -> &LedgerWriter {
        &self.writer
    }

    pub fn reserve_session(&self) -> Result<(Reservation, Reservation), Refused> {
        let mut both = self.writer.reserve_session()?;
        let close = both.split(1)?;
        Ok((both, close))
    }

    pub fn record_event(&self, content: EntryContent) {
        let event = content.event;
        let reservation = self
            .writer
            .reserve(1)
            .or_else(|_| self.writer.reserve_denial());
        match reservation {
            Ok(mut reservation) => {
                if let Err(refused) = reservation.record(content) {
                    tracing::error!(?event, %refused, "the ledger refused an event; it is not recorded");
                }
            }
            Err(refused) => {
                tracing::error!(?event, %refused, "the ledger has no room for an event; it is not recorded")
            }
        }
    }
}

impl AuditSink for LedgerAudit {
    fn reserve(&self, slots: u32) -> Result<AuditReservation, AuditRefused> {
        let mut reservation = self.writer.reserve(slots).map_err(refusal)?;
        let mut boxes: Vec<Box<dyn AuditSlot>> = Vec::with_capacity(slots as usize);
        for _ in 0..slots {
            boxes.push(Box::new(LedgerSlot {
                reservation: reservation.split(1).map_err(refusal)?,
            }));
        }
        Ok(AuditReservation::new(boxes))
    }

    fn reserve_denial(&self) -> Result<AuditReservation, AuditRefused> {
        let reservation = self.writer.reserve_denial().map_err(refusal)?;
        Ok(AuditReservation::new(vec![Box::new(LedgerSlot {
            reservation,
        })]))
    }
}

pub struct SessionFacts<'a> {
    pub device_id: &'a str,
    pub installation_id: &'a str,
    pub namespace_id: Option<u64>,
    pub epoch: Option<u64>,
}

pub fn detail(members: &[(&str, Value)]) -> Map<String, Value> {
    members
        .iter()
        .map(|(name, value)| (name.to_string(), value.clone()))
        .collect()
}

pub fn session_event(
    event: Event,
    facts: &SessionFacts<'_>,
    detail: Option<Map<String, Value>>,
) -> EntryContent {
    let mut content = EntryContent::event(event, NIGHTFALL_PRINCIPAL);
    content.device_id = Some(facts.device_id.to_string());
    content.installation_id = Some(facts.installation_id.to_string());
    content.namespace_id = facts.namespace_id;
    content.epoch = facts.epoch;
    content.event_detail = detail;
    content
}

pub fn setup_call(
    facts: &SessionFacts<'_>,
    action: &str,
    interface_id: u64,
    method_id: u16,
) -> EntryContent {
    let mut content = session_event(Event::SetupCall, facts, None);
    content.direction = Some(LedgerDirection::NightfallToNode);
    content.action = Some(action.to_string());
    content.interface_id = Some(interface_id);
    content.method_id = Some(method_id);
    content
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightfall_ledger::memory::MemoryLog;
    use nightfall_ledger::signing::CheckpointSigner;
    use nightfall_ledger::writer::{LedgerConfig, NoObserver};
    use std::sync::Arc;

    pub(crate) fn signer() -> CheckpointSigner {
        let document =
            ring::signature::Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
                .unwrap();
        CheckpointSigner::from_pkcs8_der(document.as_ref()).unwrap()
    }

    #[test]
    fn converts_every_membrane_field_into_a_ledger_entry() {
        let mut entry = AuditEntry::new(EntryKind::Call, "dawn-0");
        entry.device_id = Some("00112233445566778899aabbccddeeff".to_string());
        entry.installation_id = Some("ffeeddccbbaa99887766554433221100".to_string());
        entry.namespace_id = Some(7);
        entry.epoch = Some(42);
        entry.session_id = Some("0192f4c1-7a2b-7c3d-8e4f-0123456789ab".to_string());
        entry.call_id = Some("0192f4c1-7a2b-7c3d-8e4f-0123456789ac".to_string());
        entry.cap_id = Some(3);
        entry.parent_cap_id = Some(1);
        entry.direction = Some(Direction::ClientToNode);
        entry.action = Some("Dusk.ps".to_string());
        entry.interface_id = Some(0xe0b9);
        entry.method_id = Some(2);
        entry.param_fields = vec![nightfall_membrane::audit::ParamField {
            name: "value".to_string(),
            redacted: true,
        }];
        entry.param_hash = "ab".repeat(32);
        entry.result_code = Some(ResultCode::RateLimited);
        let converted = content(entry);
        assert_eq!(converted.kind, Kind::Call);
        assert_eq!(converted.direction, Some(LedgerDirection::ClientToNode));
        assert_eq!(converted.result_code, Some(LedgerResultCode::RateLimited));
        assert!(converted.param_fields[0].redacted);
        assert!(converted.validate().is_ok());
    }

    #[test]
    fn hands_out_one_slot_per_entry_and_resolves_a_write_ahead_entry_once_committed() {
        let log = MemoryLog::new();
        let (writer, thread) = LedgerWriter::start(
            LedgerConfig::new("nightfall-0", 0),
            Box::new(log.clone()),
            signer(),
            Arc::new(NoObserver),
        )
        .unwrap();
        let audit = LedgerAudit::new(writer.clone());
        let mut reservation = audit.reserve(2).unwrap();
        assert_eq!(reservation.slots(), 2);
        let first = reservation.split().unwrap();
        let mut call = AuditEntry::new(EntryKind::Event, "nightfall");
        call.event = Some(AuditEvent::RpcRejected);
        assert!(first.record(call).is_none());
        let facts = SessionFacts {
            device_id: "00112233445566778899aabbccddeeff",
            installation_id: "ffeeddccbbaa99887766554433221100",
            namespace_id: Some(7),
            epoch: Some(42),
        };
        audit.record_event(setup_call(&facts, "Dusk.namespaceId", 0xe0b9, 9));
        audit.record_event(session_event(
            Event::SessionRefused,
            &SessionFacts {
                epoch: None,
                namespace_id: None,
                ..facts
            },
            Some(detail(&[("reason", Value::String("revoked".to_string()))])),
        ));
        writer.shutdown();
        thread.join();
        let records = log.records();
        let events: Vec<String> = records
            .iter()
            .map(|record| {
                let value: Value = serde_json::from_slice(record).unwrap();
                value["event"].as_str().unwrap_or_default().to_string()
            })
            .collect();
        assert!(events.contains(&"rpc_rejected".to_string()));
        assert!(events.contains(&"setup_call".to_string()));
        assert!(events.contains(&"session_refused".to_string()));
    }
}
