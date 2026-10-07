use capnp::capability::Promise;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Call,
    Result,
    Event,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    ClientToNode,
    NodeToClient,
    NightfallToNode,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::ClientToNode => "client_to_node",
            Direction::NodeToClient => "node_to_client",
            Direction::NightfallToNode => "nightfall_to_node",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultCode {
    Ok,
    Failed,
    Overloaded,
    Disconnected,
    Unimplemented,
    Denied,
    RateLimited,
    Revoked,
}

impl ResultCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ResultCode::Ok => "ok",
            ResultCode::Failed => "error:failed",
            ResultCode::Overloaded => "error:overloaded",
            ResultCode::Disconnected => "error:disconnected",
            ResultCode::Unimplemented => "error:unimplemented",
            ResultCode::Denied => "denied",
            ResultCode::RateLimited => "rate_limited",
            ResultCode::Revoked => "revoked",
        }
    }

    pub fn of_error(error: &capnp::Error) -> ResultCode {
        match error.kind {
            capnp::ErrorKind::Overloaded => ResultCode::Overloaded,
            capnp::ErrorKind::Disconnected => ResultCode::Disconnected,
            capnp::ErrorKind::Unimplemented => ResultCode::Unimplemented,
            _ => ResultCode::Failed,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditEvent {
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

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Call => "call",
            EntryKind::Result => "result",
            EntryKind::Event => "event",
        }
    }
}

impl AuditEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditEvent::SessionOpen => "session_open",
            AuditEvent::SessionClose => "session_close",
            AuditEvent::SessionRefused => "session_refused",
            AuditEvent::SetupCall => "setup_call",
            AuditEvent::MembraneDropped => "membrane_dropped",
            AuditEvent::BindingConflict => "binding_conflict",
            AuditEvent::RpcRejected => "rpc_rejected",
            AuditEvent::ChainLink => "chain_link",
            AuditEvent::ChainResumed => "chain_resumed",
            AuditEvent::QuarantineOverride => "quarantine_override",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamField {
    pub name: String,
    pub redacted: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AuditEntry {
    pub kind: EntryKind,
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
    pub event: Option<AuditEvent>,
    pub event_detail: Option<serde_json::Map<String, serde_json::Value>>,
    pub write_ahead: bool,
}

impl AuditEntry {
    pub fn new(kind: EntryKind, principal: impl Into<String>) -> AuditEntry {
        AuditEntry {
            kind,
            device_id: None,
            installation_id: None,
            namespace_id: None,
            epoch: None,
            principal: principal.into(),
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
            write_ahead: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditRefused {
    pub reason: String,
}

impl fmt::Display for AuditRefused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "the ledger refused the entry: {}", self.reason)
    }
}

impl std::error::Error for AuditRefused {}

pub trait AuditSlot {
    fn record(self: Box<Self>, entry: AuditEntry) -> Option<Promise<(), capnp::Error>>;
}

pub struct AuditReservation {
    slots: Vec<Box<dyn AuditSlot>>,
}

impl AuditReservation {
    pub fn new(slots: Vec<Box<dyn AuditSlot>>) -> AuditReservation {
        AuditReservation { slots }
    }

    pub fn slots(&self) -> usize {
        self.slots.len()
    }

    pub fn split(&mut self) -> Option<AuditReservation> {
        self.slots
            .pop()
            .map(|slot| AuditReservation { slots: vec![slot] })
    }

    pub fn record(mut self, entry: AuditEntry) -> Option<Promise<(), capnp::Error>> {
        match self.slots.pop() {
            Some(slot) => slot.record(entry),
            None => {
                tracing::error!(
                    principal = %entry.principal,
                    kind = ?entry.kind,
                    action = ?entry.action,
                    "an audit entry was recorded on an empty reservation and is lost"
                );
                None
            }
        }
    }
}

pub trait AuditSink {
    fn reserve(&self, slots: u32) -> Result<AuditReservation, AuditRefused>;
    fn reserve_denial(&self) -> Result<AuditReservation, AuditRefused>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Recorder {
        recorded: Rc<RefCell<Vec<AuditEntry>>>,
    }

    impl AuditSlot for Recorder {
        fn record(self: Box<Self>, entry: AuditEntry) -> Option<Promise<(), capnp::Error>> {
            let write_ahead = entry.write_ahead;
            self.recorded.borrow_mut().push(entry);
            write_ahead.then(|| Promise::ok(()))
        }
    }

    #[test]
    fn splits_a_reservation_into_single_slots_and_records_each_once() {
        let recorded = Rc::new(RefCell::new(Vec::new()));
        let slots: Vec<Box<dyn AuditSlot>> = (0..2)
            .map(|_| {
                Box::new(Recorder {
                    recorded: recorded.clone(),
                }) as Box<dyn AuditSlot>
            })
            .collect();
        let mut reservation = AuditReservation::new(slots);
        assert_eq!(reservation.slots(), 2);
        let call = reservation.split().unwrap();
        let result = reservation.split().unwrap();
        assert!(reservation.split().is_none());
        let mut entry = AuditEntry::new(EntryKind::Call, "dawn-0");
        entry.write_ahead = true;
        assert!(call.record(entry).is_some());
        assert!(
            result
                .record(AuditEntry::new(EntryKind::Result, "dawn-0"))
                .is_none()
        );
        assert!(
            reservation
                .record(AuditEntry::new(EntryKind::Event, "dawn-0"))
                .is_none()
        );
        assert_eq!(recorded.borrow().len(), 2);
    }

    #[test]
    fn spells_result_codes_and_directions_as_the_ledger_contract_does() {
        assert_eq!(ResultCode::Ok.as_str(), "ok");
        assert_eq!(ResultCode::RateLimited.as_str(), "rate_limited");
        assert_eq!(ResultCode::Unimplemented.as_str(), "error:unimplemented");
        assert_eq!(
            ResultCode::of_error(&capnp::Error::disconnected(String::new())),
            ResultCode::Disconnected
        );
        assert_eq!(
            ResultCode::of_error(&capnp::Error::failed(String::new())),
            ResultCode::Failed
        );
        assert_eq!(Direction::NodeToClient.as_str(), "node_to_client");
        assert_eq!(
            AuditEvent::QuarantineOverride.as_str(),
            "quarantine_override"
        );
        assert_eq!(EntryKind::Result.as_str(), "result");
    }
}
