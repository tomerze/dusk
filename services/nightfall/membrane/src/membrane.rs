use crate::audit::{
    AuditEntry, AuditEvent, AuditReservation, AuditSink, Direction, EntryKind, ParamField,
    ResultCode,
};
use crate::canonical::{Sanitized, param_hash, sanitize};
use crate::copy::{Scratch, merge_into, path_operations, place_capability};
use crate::gate::{Gate, GateTicket};
use crate::limits::{
    CallPermit, CapPermit, Lane, LimitExceeded, LimitState, SessionLimits, TokenBucket,
};
use crate::node::NodeLink;
use crate::permissions::{Decision, Permissions, Policy};
use crate::provenance::{Provenance, ProvenanceEntry};
use crate::schema::{Bundle, MethodSchema};
use capnp::any_pointer;
use capnp::capability::{
    DispatchCallResult, FromClientHook, FromServer, Params, Promise, Results, Server,
};
use capnp::private::capability::ClientHook;
use capnp::private::layout::{CapTable, StructSize};
use capnp::traits::HasTypeId;
use dusk_capnp::dusk_capnp::{dusk, program_args};
use futures::channel::oneshot;
use futures::future::{Either, select};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub const BOOTSTRAP_CAP_ID: u64 = 0;
pub const NO_PROCESS: u64 = 0;
pub const MAXIMUM_CAPABILITIES_PER_MESSAGE: usize = 10_000;

const PROCESS_METHOD: u16 = 0;
const RUN_METHOD: u16 = 1;
const KILL_METHOD: u16 = 4;
const WAITPID_METHOD: u16 = 5;

fn dusk_interface() -> u64 {
    <dusk::Client as HasTypeId>::TYPE_ID
}

fn revoked() -> capnp::Error {
    capnp::Error::disconnected("session revoked".to_string())
}

fn rate_limited(limit: &str) -> capnp::Error {
    capnp::Error::overloaded(format!("rate limited: {limit}"))
}

async fn until_revoked<T>(
    revocation: &CancellationToken,
    future: impl Future<Output = T>,
) -> Result<T, capnp::Error> {
    match select(Box::pin(future), Box::pin(revocation.cancelled())).await {
        Either::Left((value, _)) => Ok(value),
        Either::Right(_) => Err(revoked()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Node,
    Client,
}

impl Side {
    fn opposite(self) -> Side {
        match self {
            Side::Node => Side::Client,
            Side::Client => Side::Node,
        }
    }
}

struct Scope {
    hooks: RefCell<HashMap<u64, Box<dyn ClientHook>>>,
    revocation: CancellationToken,
}

impl Drop for Scope {
    fn drop(&mut self) {
        self.revocation.cancel();
    }
}

struct Capability {
    cap_id: u64,
    parent_cap_id: Option<u64>,
    pid: u64,
    side: Side,
    scope: Weak<Scope>,
    connection: Weak<ConnectionState>,
    memo_pointer: Cell<usize>,
    gate: Rc<Gate>,
    _permit: CapPermit,
}

impl Drop for Capability {
    fn drop(&mut self) {
        if let Some(scope) = self.scope.upgrade() {
            let removed = match scope.hooks.try_borrow_mut() {
                Ok(mut hooks) => hooks.remove(&self.cap_id),
                Err(_) => {
                    tracing::error!(
                        cap_id = self.cap_id,
                        "a capability was released while its scope table was in use"
                    );
                    None
                }
            };
            drop(removed);
        }
        if let Some(connection) = self.connection.upgrade() {
            connection.forget(self.memo_pointer.get(), self.cap_id);
        }
    }
}

struct Wrapper {
    capability: Rc<Capability>,
    _alive: Rc<()>,
}

struct Registration {
    alive: Weak<()>,
    capability: Weak<Capability>,
}

struct WrapperDispatch {
    wrapper: Wrapper,
}

impl Deref for WrapperDispatch {
    type Target = Wrapper;

    fn deref(&self) -> &Wrapper {
        &self.wrapper
    }
}

impl DerefMut for WrapperDispatch {
    fn deref_mut(&mut self) -> &mut Wrapper {
        &mut self.wrapper
    }
}

impl Drop for WrapperDispatch {
    fn drop(&mut self) {
        let pointer = self as *const WrapperDispatch as usize;
        let Some(connection) = self.wrapper.capability.connection.upgrade() else {
            return;
        };
        match connection.wrappers.try_borrow_mut() {
            Ok(mut wrappers) => {
                wrappers.remove(&pointer);
            }
            Err(_) => tracing::error!(
                cap_id = self.wrapper.capability.cap_id,
                "a wrapper was released while the wrapper table was in use; its entry stays until the address is reused"
            ),
        }
    }
}

impl Server for WrapperDispatch {
    fn dispatch_call(
        &mut self,
        interface_id: u64,
        method_id: u16,
        params: Params<any_pointer::Owned>,
        results: Results<any_pointer::Owned>,
    ) -> DispatchCallResult {
        let capability = &self.wrapper.capability;
        let Some(connection) = capability.connection.upgrade() else {
            return DispatchCallResult::new(Promise::err(revoked()), false);
        };
        let target = Target {
            cap_id: capability.cap_id,
            parent_cap_id: capability.parent_cap_id,
            pid: capability.pid,
            side: capability.side,
            scope: capability.scope.clone(),
            gate: capability.gate.clone(),
            bootstrap: false,
        };
        connection.dispatch(target, interface_id, method_id, params, results)
    }
}

struct WrapperClient {
    hook: Box<dyn ClientHook>,
}

impl FromClientHook for WrapperClient {
    fn new(hook: Box<dyn ClientHook>) -> WrapperClient {
        WrapperClient { hook }
    }

    fn into_client_hook(self) -> Box<dyn ClientHook> {
        self.hook
    }

    fn as_client_hook(&self) -> &dyn ClientHook {
        self.hook.as_ref()
    }
}

impl FromServer<Wrapper> for WrapperClient {
    type Dispatch = WrapperDispatch;

    fn from_server(wrapper: Wrapper) -> WrapperDispatch {
        WrapperDispatch { wrapper }
    }
}

struct Bootstrap {
    connection: Weak<ConnectionState>,
}

impl Server for Box<Bootstrap> {
    fn dispatch_call(
        &mut self,
        interface_id: u64,
        method_id: u16,
        params: Params<any_pointer::Owned>,
        results: Results<any_pointer::Owned>,
    ) -> DispatchCallResult {
        let Some(connection) = self.connection.upgrade() else {
            return DispatchCallResult::new(Promise::err(revoked()), false);
        };
        let scope = connection
            .root
            .borrow()
            .as_ref()
            .map(Rc::downgrade)
            .unwrap_or_default();
        let target = Target {
            cap_id: BOOTSTRAP_CAP_ID,
            parent_cap_id: None,
            pid: NO_PROCESS,
            side: Side::Node,
            scope,
            gate: connection.bootstrap_gate.clone(),
            bootstrap: true,
        };
        connection.dispatch(target, interface_id, method_id, params, results)
    }
}

struct BootstrapClient {
    hook: Box<dyn ClientHook>,
}

impl FromClientHook for BootstrapClient {
    fn new(hook: Box<dyn ClientHook>) -> BootstrapClient {
        BootstrapClient { hook }
    }

    fn into_client_hook(self) -> Box<dyn ClientHook> {
        self.hook
    }

    fn as_client_hook(&self) -> &dyn ClientHook {
        self.hook.as_ref()
    }
}

impl FromServer<Bootstrap> for BootstrapClient {
    type Dispatch = Box<Bootstrap>;

    fn from_server(bootstrap: Bootstrap) -> Box<Bootstrap> {
        Box::new(bootstrap)
    }
}

#[derive(Clone)]
struct Target {
    cap_id: u64,
    parent_cap_id: Option<u64>,
    pid: u64,
    side: Side,
    scope: Weak<Scope>,
    gate: Rc<Gate>,
    bootstrap: bool,
}

#[derive(Clone)]
struct CallInfo {
    call_id: String,
    cap_id: u64,
    parent_cap_id: Option<u64>,
    direction: Direction,
    action: String,
    interface_id: u64,
    method_id: u16,
    pid: u64,
    started: Instant,
}

struct Prepared {
    target: Target,
    call: CallInfo,
    method: MethodSchema,
    bundle: Arc<Bundle>,
    scratch: Scratch,
    sanitized: Sanitized,
    param_hash: String,
    decision: Decision,
    permit: CallPermit,
    lane: Lane,
    waits: bool,
    needs_token: bool,
    revocation: CancellationToken,
}

struct ResultGuard {
    connection: Rc<ConnectionState>,
    call: CallInfo,
    slot: Option<AuditReservation>,
    revocation: CancellationToken,
}

impl ResultGuard {
    fn record(mut self, code: ResultCode, cap_ids: Vec<u64>) {
        if let Some(slot) = self.slot.take() {
            self.connection
                .record_result(&self.call, slot, code, cap_ids);
        }
    }
}

impl Drop for ResultGuard {
    fn drop(&mut self) {
        let Some(slot) = self.slot.take() else {
            return;
        };
        let code = if self.revocation.is_cancelled() || self.connection.dropped.get() {
            ResultCode::Revoked
        } else {
            ResultCode::Disconnected
        };
        tracing::debug!(
            principal = %self.connection.principal,
            session_id = %self.connection.session_id,
            call_id = %self.call.call_id,
            action = %self.call.action,
            result = code.as_str(),
            "a forwarded call ended before its answer"
        );
        self.connection
            .record_result(&self.call, slot, code, Vec::new());
    }
}

struct PendingPath {
    pointers: Vec<u16>,
    capability: Rc<Capability>,
    wrapper: Box<dyn ClientHook>,
    resolver: oneshot::Sender<Result<Box<dyn ClientHook>, capnp::Error>>,
}

struct FilledPath {
    pointers: Vec<u16>,
    capability: Rc<Capability>,
    wrapper: Box<dyn ClientHook>,
}

#[derive(Default)]
struct Pipeline {
    paths: Vec<PendingPath>,
}

impl Pipeline {
    fn fail(self, error: &capnp::Error) {
        for path in self.paths {
            if path.resolver.send(Err(error.clone())).is_err() {
                tracing::debug!(
                    cap_id = path.capability.cap_id,
                    "a pipelined capability of a failed call was already released"
                );
            }
        }
    }
}

struct Begun {
    prepared: Prepared,
    mapped: CapTable,
    result: ResultGuard,
    commit: Option<Promise<(), capnp::Error>>,
}

struct InFlight {
    prepared: Prepared,
    result: ResultGuard,
    promise: Promise<capnp::capability::Response<any_pointer::Owned>, capnp::Error>,
    paths: Vec<FilledPath>,
}

enum MapFailure {
    Limit(LimitExceeded),
    Refused(capnp::Error),
}

struct ConnectionState {
    node: Rc<dyn NodeLink>,
    principal: String,
    policy: Policy,
    bundle: Arc<Bundle>,
    audit: Rc<dyn AuditSink>,
    param_key: Rc<[u8]>,
    limits: Rc<LimitState>,
    session_limits: Rc<SessionLimits>,
    bucket: Rc<RefCell<TokenBucket>>,
    refusals: RefCell<TokenBucket>,
    session_id: String,
    next_cap_id: Cell<u64>,
    root: RefCell<Option<Rc<Scope>>>,
    wrappers: RefCell<HashMap<usize, Registration>>,
    memo: RefCell<HashMap<usize, Weak<Capability>>>,
    provenance: RefCell<Provenance>,
    revocation: CancellationToken,
    dropped: Cell<bool>,
    bootstrap_pointer: Cell<usize>,
    bootstrap_gate: Rc<Gate>,
}

impl ConnectionState {
    fn forget(&self, pointer: usize, cap_id: u64) {
        match self.memo.try_borrow_mut() {
            Ok(mut memo) => {
                if memo
                    .get(&pointer)
                    .is_some_and(|capability| capability.strong_count() == 0)
                {
                    memo.remove(&pointer);
                }
            }
            Err(_) => tracing::error!(
                cap_id,
                "a capability was released while the memo table was in use"
            ),
        }
        match self.provenance.try_borrow_mut() {
            Ok(mut provenance) => provenance.release(cap_id),
            Err(_) => tracing::error!(
                cap_id,
                "a capability was released while the provenance table was in use"
            ),
        }
    }

    fn base_entry(&self, kind: EntryKind, call: &CallInfo) -> AuditEntry {
        let identity = self.node.identity();
        let mut entry = AuditEntry::new(kind, self.principal.clone());
        entry.device_id = Some(identity.device_id.clone());
        entry.installation_id = Some(identity.installation_id.clone());
        entry.namespace_id = Some(identity.namespace_id);
        entry.epoch = Some(self.node.epoch());
        entry.pid = call.pid;
        entry.session_id = Some(self.session_id.clone());
        entry.call_id = Some(call.call_id.clone());
        entry.cap_id = Some(call.cap_id);
        entry.parent_cap_id = call.parent_cap_id;
        entry.direction = Some(call.direction);
        entry.action = Some(call.action.clone());
        entry.interface_id = Some(call.interface_id);
        entry.method_id = Some(call.method_id);
        entry
    }

    fn event_entry(&self, event: AuditEvent, pid: u64) -> AuditEntry {
        let identity = self.node.identity();
        let mut entry = AuditEntry::new(EntryKind::Event, self.principal.clone());
        entry.device_id = Some(identity.device_id.clone());
        entry.installation_id = Some(identity.installation_id.clone());
        entry.namespace_id = Some(identity.namespace_id);
        entry.epoch = Some(self.node.epoch());
        entry.pid = pid;
        entry.session_id = Some(self.session_id.clone());
        entry.event = Some(event);
        entry
    }

    fn record_event(&self, entry: AuditEntry) {
        let reservation = self
            .audit
            .reserve(1)
            .or_else(|_| self.audit.reserve_denial());
        match reservation {
            Ok(reservation) => {
                if let Some(commit) = reservation.record(entry) {
                    drop(commit);
                }
            }
            Err(refused) => tracing::error!(
                principal = %self.principal,
                session_id = %self.session_id,
                event = ?entry.event,
                reason = %refused.reason,
                "the ledger refused an event entry; the event is not recorded"
            ),
        }
    }

    fn count_call(call: &CallInfo, code: ResultCode) {
        let action = if call.action.starts_with("unknown:") {
            "unknown".to_string()
        } else {
            call.action.clone()
        };
        metrics::counter!(
            "nightfall_calls_total",
            "direction" => call.direction.as_str(),
            "action" => action,
            "result" => code.as_str()
        )
        .increment(1);
    }

    fn record_rejection(
        &self,
        call: &CallInfo,
        code: ResultCode,
        parameters: impl FnOnce() -> (Vec<ParamField>, String),
    ) {
        Self::count_call(call, code);
        if !self.refusals.borrow_mut().try_take(Instant::now()) {
            metrics::counter!("nightfall_unrecorded_refusals_total", "result" => code.as_str())
                .increment(1);
            tracing::debug!(
                principal = %self.principal,
                session_id = %self.session_id,
                call_id = %call.call_id,
                action = %call.action,
                result = code.as_str(),
                "a refused call is beyond the session's refusal budget; it is not ledgered"
            );
            return;
        }
        let (fields, hash) = parameters();
        let mut entry = self.base_entry(EntryKind::Call, call);
        entry.param_fields = fields;
        entry.param_hash = hash;
        entry.result_code = Some(code);
        match self.audit.reserve_denial() {
            Ok(reservation) => {
                if let Some(commit) = reservation.record(entry) {
                    drop(commit);
                }
            }
            Err(refused) => tracing::error!(
                principal = %self.principal,
                session_id = %self.session_id,
                call_id = %call.call_id,
                action = %call.action,
                result = code.as_str(),
                reason = %refused.reason,
                "the ledger refused a rejected call's entry; the rejection is not recorded"
            ),
        }
    }

    fn refuse(
        &self,
        call: &CallInfo,
        limit: &'static str,
        parameters: impl FnOnce() -> (Vec<ParamField>, String),
    ) -> capnp::Error {
        metrics::counter!("nightfall_rate_limited_total", "limit" => limit).increment(1);
        tracing::debug!(
            principal = %self.principal,
            session_id = %self.session_id,
            call_id = %call.call_id,
            action = %call.action,
            limit,
            "call rejected by a limit"
        );
        self.record_rejection(call, ResultCode::RateLimited, parameters);
        rate_limited(limit)
    }

    fn refuse_prepared(&self, prepared: &Prepared, limit: &'static str) -> capnp::Error {
        self.refuse(&prepared.call, limit, || {
            (
                prepared.sanitized.fields.clone(),
                prepared.param_hash.clone(),
            )
        })
    }

    fn record_result(
        &self,
        call: &CallInfo,
        slot: AuditReservation,
        code: ResultCode,
        cap_ids: Vec<u64>,
    ) {
        let mut entry = self.base_entry(EntryKind::Result, call);
        entry.result_code = Some(code);
        entry.result_cap_ids = cap_ids;
        Self::count_call(call, code);
        metrics::histogram!("nightfall_call_duration_seconds")
            .record(call.started.elapsed().as_secs_f64());
        if let Some(commit) = slot.record(entry) {
            drop(commit);
        }
    }

    fn resolve(
        &self,
        interface_id: u64,
        method_id: u16,
    ) -> Option<(Arc<Bundle>, MethodSchema, String)> {
        let bundle = self.bundle.clone();
        let resolved = bundle.method(interface_id, method_id)?;
        let action = resolved.action();
        let method = resolved.method.clone();
        Some((bundle, method, action))
    }

    fn dispatch(
        self: &Rc<Self>,
        target: Target,
        interface_id: u64,
        method_id: u16,
        params: Params<any_pointer::Owned>,
        results: Results<any_pointer::Owned>,
    ) -> DispatchCallResult {
        let streaming = self
            .resolve(interface_id, method_id)
            .is_some_and(|(_, method, _)| method.streaming);
        let promise = match self.prepare(target, interface_id, method_id, params) {
            Ok(prepared) => self.clone().run(prepared, results),
            Err(error) => Promise::err(error),
        };
        DispatchCallResult::new(promise, streaming)
    }

    fn bucket(&self, lane: Lane) -> &RefCell<TokenBucket> {
        match lane {
            Lane::Forward => &self.bucket,
            Lane::Reverse => self.session_limits.reverse_bucket(),
        }
    }

    fn decide(
        &self,
        target: &Target,
        interface_id: u64,
        direction: Direction,
        action: &str,
    ) -> Decision {
        let (interface_name, method_name) = action.rsplit_once('.').unwrap_or((action, ""));
        if target.bootstrap && interface_id != dusk_interface() {
            return Decision::Denied;
        }
        match direction {
            Direction::NodeToClient => self.policy.reverse_allows(interface_name, method_name),
            _ => self.policy.allows(interface_name, method_name),
        }
    }

    fn returns_home(&self, hook: &dyn ClientHook, destination: Side) -> bool {
        let mut hook = hook.add_ref();
        while let Some(resolved) = hook.get_resolved() {
            hook = resolved;
        }
        let pointer = hook.get_ptr();
        if pointer == self.bootstrap_pointer.get() {
            return destination == Side::Node;
        }
        self.wrappers
            .borrow()
            .get(&pointer)
            .filter(|registration| registration.alive.strong_count() > 0)
            .and_then(|registration| registration.capability.upgrade())
            .is_some_and(|capability| capability.side == destination)
    }

    fn pid_of(&self, mut hook: Box<dyn ClientHook>) -> Option<u64> {
        while let Some(resolved) = hook.get_resolved() {
            hook = resolved;
        }
        self.wrappers
            .borrow()
            .get(&hook.get_ptr())
            .filter(|registration| registration.alive.strong_count() > 0)
            .and_then(|registration| registration.capability.upgrade())
            .map(|capability| capability.pid)
    }

    fn call_pid(
        &self,
        target: &Target,
        interface_id: u64,
        method_id: u16,
        params: &Params<any_pointer::Owned>,
    ) -> u64 {
        if interface_id != dusk_interface() {
            return target.pid;
        }
        let Ok(parameters) = params.get() else {
            return target.pid;
        };
        let named = match method_id {
            PROCESS_METHOD => parameters
                .get_as::<dusk::process_params::Reader>()
                .and_then(|parameters| parameters.get_program_args())
                .ok()
                .and_then(|program_args| program_args.get_pid().which().ok())
                .and_then(|pid| match pid {
                    program_args::pid::Fixed(pid) => Some(pid),
                    program_args::pid::Auto(()) => None,
                }),
            RUN_METHOD => parameters
                .get_as::<dusk::run_params::Reader>()
                .and_then(|parameters| parameters.get_process())
                .ok()
                .and_then(|process| self.pid_of(process.client.hook)),
            KILL_METHOD => parameters
                .get_as::<dusk::kill_params::Reader>()
                .ok()
                .map(|parameters| parameters.get_pid()),
            WAITPID_METHOD => parameters
                .get_as::<dusk::waitpid_params::Reader>()
                .ok()
                .map(|parameters| parameters.get_pid()),
            _ => None,
        };
        named.filter(|pid| *pid != NO_PROCESS).unwrap_or(target.pid)
    }

    fn prepare(
        &self,
        target: Target,
        interface_id: u64,
        method_id: u16,
        params: Params<any_pointer::Owned>,
    ) -> Result<Prepared, capnp::Error> {
        if self.dropped.get() {
            return Err(revoked());
        }
        let Some(scope) = target.scope.upgrade() else {
            return Err(revoked());
        };
        if self.node.closed() {
            return Err(capnp::Error::disconnected(
                "node session closed".to_string(),
            ));
        }
        let (direction, lane) = match target.side {
            Side::Node => (Direction::ClientToNode, Lane::Forward),
            Side::Client => (Direction::NodeToClient, Lane::Reverse),
        };
        let mut call = CallInfo {
            call_id: uuid::Uuid::now_v7().to_string(),
            cap_id: target.cap_id,
            parent_cap_id: target.parent_cap_id,
            direction,
            action: format!("unknown:{interface_id:016x}.{method_id}"),
            interface_id,
            method_id,
            pid: target.pid,
            started: Instant::now(),
        };
        let resolved = self.resolve(interface_id, method_id);
        let token = self.limits.try_token(self.bucket(lane), lane);
        let Some((bundle, method, action)) = resolved else {
            if let Err(exceeded) = token {
                return Err(self.refuse(&call, exceeded.limit, || (Vec::new(), String::new())));
            }
            tracing::warn!(
                principal = %self.principal,
                session_id = %self.session_id,
                call_id = %call.call_id,
                interface_id = format!("{interface_id:016x}"),
                method_id,
                bundle = %self.bundle.name(),
                "call on an interface the schema bundle does not know"
            );
            self.record_rejection(&call, ResultCode::Denied, || (Vec::new(), String::new()));
            return Err(capnp::Error::unimplemented(format!(
                "unknown interface {interface_id:016x} method {method_id}"
            )));
        };
        call.action = action;
        let decision = self.decide(&target, interface_id, direction, &call.action);
        let waits = method.streaming || direction == Direction::NodeToClient;
        if let Err(exceeded) = token
            && (decision == Decision::Denied || !waits)
        {
            return Err(self.refuse(&call, exceeded.limit, || (Vec::new(), String::new())));
        }
        call.pid = self.call_pid(&target, interface_id, method_id, &params);
        let copy = |params: Params<any_pointer::Owned>| {
            params.get().and_then(Scratch::copy).and_then(|scratch| {
                let sanitized =
                    sanitize(&bundle, Some(method.param_struct), scratch.reader()?, true)?;
                Ok((scratch, sanitized))
            })
        };
        if decision == Decision::Denied {
            tracing::info!(
                principal = %self.principal,
                session_id = %self.session_id,
                call_id = %call.call_id,
                action = %call.action,
                direction = direction.as_str(),
                pid = call.pid,
                "call denied by permissions"
            );
            self.record_rejection(&call, ResultCode::Denied, || match copy(params) {
                Ok((_, sanitized)) => {
                    let hash = param_hash(&self.param_key, &sanitized.canonical);
                    (sanitized.fields, hash)
                }
                Err(_) => (Vec::new(), String::new()),
            });
            return Err(capnp::Error::unimplemented(format!(
                "not permitted: {}",
                call.action
            )));
        }
        let copied = copy(params);
        let (scratch, sanitized) = match copied {
            Ok(copied) => copied,
            Err(error) => {
                tracing::warn!(
                    principal = %self.principal,
                    session_id = %self.session_id,
                    call_id = %call.call_id,
                    action = %call.action,
                    error = %error,
                    "call refused: its parameters could not be read"
                );
                self.record_rejection(&call, ResultCode::Denied, || (Vec::new(), String::new()));
                return Err(error);
            }
        };
        let hash = param_hash(&self.param_key, &sanitized.canonical);
        let bytes = scratch.bytes().saturating_mul(2);
        let permit = match self.limits.hold(&self.session_limits, bytes) {
            Ok(permit) => permit,
            Err(exceeded) => {
                return Err(self.refuse(&call, exceeded.limit, || (sanitized.fields, hash)));
            }
        };
        Ok(Prepared {
            target,
            call,
            method,
            bundle,
            scratch,
            sanitized,
            param_hash: hash,
            decision,
            permit,
            lane,
            waits,
            needs_token: token.is_err(),
            revocation: scope.revocation.clone(),
        })
    }

    fn pipeline(
        self: &Rc<Self>,
        prepared: &Prepared,
        results: &mut Results<any_pointer::Owned>,
    ) -> Result<Pipeline, capnp::Error> {
        let mut pipeline = Pipeline::default();
        if prepared.method.pipeline_paths.is_empty() {
            return Ok(pipeline);
        }
        let Some(scope) = prepared.target.scope.upgrade() else {
            return Err(revoked());
        };
        for path in &prepared.method.pipeline_paths {
            let (resolver, resolution) = oneshot::channel();
            let promised: capnp::capability::Client = capnp_rpc::new_future_client(async move {
                match resolution.await {
                    Ok(Ok(hook)) => Ok(capnp::capability::Client::new(hook)),
                    Ok(Err(error)) => Err(error),
                    Err(oneshot::Canceled) => Err(capnp::Error::disconnected(
                        "the call was not forwarded".to_string(),
                    )),
                }
            });
            let capability = match self.new_capability(
                &scope,
                prepared.target.side,
                promised.hook,
                path.interface_id,
                &prepared.call,
            ) {
                Ok(capability) => capability,
                Err(exceeded) => {
                    tracing::warn!(
                        principal = %self.principal,
                        session_id = %self.session_id,
                        call_id = %prepared.call.call_id,
                        limit = exceeded.limit,
                        "no pipelined capability for a result path; the live capability bound is reached"
                    );
                    continue;
                }
            };
            let minted = self.mint(&capability);
            let sizes: Vec<StructSize> = path
                .structs
                .iter()
                .zip(&path.pointers)
                .map(|(struct_id, pointer)| {
                    let size = prepared
                        .bundle
                        .structure(*struct_id)
                        .map(|structure| structure.size())
                        .unwrap_or(StructSize {
                            data: 0,
                            pointers: 0,
                        });
                    StructSize {
                        data: size.data,
                        pointers: size.pointers.max(pointer + 1),
                    }
                })
                .collect();
            place_capability(results.get(), &path.pointers, &sizes, minted.add_ref())?;
            pipeline.paths.push(PendingPath {
                pointers: path.pointers.clone(),
                capability,
                wrapper: minted,
                resolver,
            });
        }
        if !pipeline.paths.is_empty() {
            results.set_pipeline()?;
        }
        Ok(pipeline)
    }

    fn run(
        self: Rc<Self>,
        prepared: Prepared,
        mut results: Results<any_pointer::Owned>,
    ) -> Promise<(), capnp::Error> {
        let pipeline = match self.pipeline(&prepared, &mut results) {
            Ok(pipeline) => pipeline,
            Err(error) => {
                tracing::warn!(
                    principal = %self.principal,
                    session_id = %self.session_id,
                    call_id = %prepared.call.call_id,
                    action = %prepared.call.action,
                    %error,
                    "the pipeline of a call could not be set up; it is not forwarded"
                );
                return Promise::err(error);
            }
        };
        let pipelined = !pipeline.paths.is_empty();
        let call_id = prepared.call.call_id.clone();
        let entered = prepared.target.gate.enter();
        let forwarding = async move {
            let mut pipeline = Some(pipeline);
            let outcome = self
                .forward(prepared, entered, &mut pipeline, &mut results)
                .await;
            if let (Err(error), Some(pipeline)) = (&outcome, pipeline) {
                pipeline.fail(error);
            }
            drop(results);
            outcome
        };
        if !pipelined {
            return Promise::from_future(forwarding);
        }
        let (sender, receiver) = oneshot::channel();
        tokio::task::spawn_local(async move {
            if sender.send(forwarding.await).is_err() {
                tracing::debug!(
                    call_id = %call_id,
                    "a call whose result was pipelined ended after its caller let go of it"
                );
            }
        });
        Promise::from_future(async move {
            receiver.await.unwrap_or_else(|_| {
                Err(capnp::Error::disconnected(
                    "the forwarding of the call ended".to_string(),
                ))
            })
        })
    }

    async fn forward(
        self: &Rc<Self>,
        prepared: Prepared,
        entered: impl Future<Output = GateTicket>,
        pipeline: &mut Option<Pipeline>,
        results: &mut Results<any_pointer::Owned>,
    ) -> Result<(), capnp::Error> {
        let deadline =
            Instant::now() + Duration::from_millis(self.limits.limits().stream_wait_timeout_ms);
        let entered = tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), entered);
        let ticket = match until_revoked(&prepared.revocation, entered).await? {
            Ok(ticket) => ticket,
            Err(_) => return Err(self.refuse_prepared(&prepared, "stream_wait_timeout_ms")),
        };
        self.admit(&prepared, deadline).await?;
        let begun = self.begin(prepared)?;
        let begun = self.wait_commit(begun).await?;
        let in_flight = self.send(begun, pipeline.take().unwrap_or_default());
        drop(ticket);
        self.finish(in_flight?, results).await
    }

    async fn admit(&self, prepared: &Prepared, deadline: Instant) -> Result<(), capnp::Error> {
        if prepared.needs_token {
            let waited = self
                .limits
                .token(self.bucket(prepared.lane), prepared.lane, deadline);
            if let Err(exceeded) = until_revoked(&prepared.revocation, waited).await? {
                return Err(self.refuse_prepared(prepared, exceeded.limit));
            }
        }
        match self.limits.try_slot(&prepared.permit, prepared.lane) {
            Ok(()) => return Ok(()),
            Err(exceeded) if !prepared.waits => {
                return Err(self.refuse_prepared(prepared, exceeded.limit));
            }
            Err(_) => {}
        }
        let waited = self.limits.slot(&prepared.permit, prepared.lane, deadline);
        if let Err(exceeded) = until_revoked(&prepared.revocation, waited).await? {
            return Err(self.refuse_prepared(prepared, exceeded.limit));
        }
        Ok(())
    }

    fn begin(self: &Rc<Self>, mut prepared: Prepared) -> Result<Begun, capnp::Error> {
        let Some(scope) = prepared.target.scope.upgrade() else {
            return Err(revoked());
        };
        if self.dropped.get() {
            return Err(revoked());
        }
        let table = prepared.scratch.take_table();
        let (mapped, param_cap_ids) = match self.map_table(
            &scope,
            table,
            prepared.target.side,
            &prepared.call,
            &prepared.sanitized.interfaces,
            &[],
        ) {
            Ok(mapped) => mapped,
            Err(MapFailure::Limit(exceeded)) => {
                return Err(self.refuse_prepared(&prepared, exceeded.limit));
            }
            Err(MapFailure::Refused(error)) => {
                self.record_rejection(&prepared.call, ResultCode::Denied, || {
                    (
                        prepared.sanitized.fields.clone(),
                        prepared.param_hash.clone(),
                    )
                });
                return Err(error);
            }
        };
        drop(scope);
        let override_event = prepared.decision == Decision::AllowedByOverride;
        let slots = 2 + u32::from(override_event);
        let mut reservation = self.audit.reserve(slots).map_err(|refused| {
            tracing::warn!(
                principal = %self.principal,
                session_id = %self.session_id,
                call_id = %prepared.call.call_id,
                action = %prepared.call.action,
                reason = %refused.reason,
                "the ledger refused a call; it is not forwarded"
            );
            capnp::Error::overloaded(refused.reason.clone())
        })?;
        let (Some(call_slot), Some(result_slot)) = (reservation.split(), reservation.split())
        else {
            return Err(capnp::Error::failed(
                "the ledger returned a reservation with fewer slots than asked for".to_string(),
            ));
        };
        let override_slot = if override_event {
            reservation.split()
        } else {
            None
        };
        let mut entry = self.base_entry(EntryKind::Call, &prepared.call);
        entry.param_fields = prepared.sanitized.fields.clone();
        entry.param_cap_ids = param_cap_ids;
        entry.param_hash = prepared.param_hash.clone();
        entry.write_ahead =
            prepared.call.direction == Direction::ClientToNode && !prepared.method.streaming;
        let commit = call_slot.record(entry);
        let result = ResultGuard {
            connection: self.clone(),
            call: prepared.call.clone(),
            slot: Some(result_slot),
            revocation: prepared.revocation.clone(),
        };
        if override_event {
            let mut event = self.event_entry(AuditEvent::QuarantineOverride, prepared.call.pid);
            event.call_id = Some(prepared.call.call_id.clone());
            event.action = Some(prepared.call.action.clone());
            event.cap_id = Some(prepared.call.cap_id);
            tracing::warn!(
                principal = %self.principal,
                session_id = %self.session_id,
                call_id = %prepared.call.call_id,
                action = %prepared.call.action,
                "call on a quarantined node allowed by a quarantine override"
            );
            if let Some(slot) = override_slot
                && let Some(commit) = slot.record(event)
            {
                drop(commit);
            }
        }
        Ok(Begun {
            prepared,
            mapped,
            result,
            commit,
        })
    }

    async fn wait_commit(&self, mut begun: Begun) -> Result<Begun, capnp::Error> {
        let Some(commit) = begun.commit.take() else {
            return Ok(begun);
        };
        let budget = Duration::from_millis(self.limits.limits().write_ahead_timeout_ms);
        let failure = match tokio::time::timeout(budget, commit).await {
            Ok(Ok(())) => return Ok(begun),
            Ok(Err(error)) => capnp::Error::overloaded(error.extra),
            Err(_) => {
                capnp::Error::overloaded("the ledger did not commit the call in time".to_string())
            }
        };
        tracing::warn!(
            principal = %self.principal,
            session_id = %self.session_id,
            call_id = %begun.prepared.call.call_id,
            action = %begun.prepared.call.action,
            error = %failure,
            "a write-ahead call entry did not commit; the call is not forwarded"
        );
        begun.result.record(ResultCode::Overloaded, Vec::new());
        Err(failure)
    }

    fn inner_hook(&self, scope: &Scope, target: &Target) -> Option<Box<dyn ClientHook>> {
        let existing = scope
            .hooks
            .borrow()
            .get(&target.cap_id)
            .map(|hook| hook.add_ref());
        if existing.is_some() || !target.bootstrap {
            return existing;
        }
        let fresh: dusk::Client = capnp_rpc::new_future_client(self.node.fresh_dusk());
        let hook = fresh.into_client_hook();
        scope
            .hooks
            .borrow_mut()
            .insert(target.cap_id, hook.add_ref());
        Some(hook)
    }

    fn send(self: &Rc<Self>, begun: Begun, pipeline: Pipeline) -> Result<InFlight, capnp::Error> {
        let Begun {
            prepared,
            mapped,
            result,
            commit: _,
        } = begun;
        let Some(scope) = prepared
            .target
            .scope
            .upgrade()
            .filter(|_| !self.dropped.get())
        else {
            result.record(ResultCode::Revoked, Vec::new());
            pipeline.fail(&revoked());
            return Err(revoked());
        };
        let Some(inner) = self.inner_hook(&scope, &prepared.target) else {
            result.record(ResultCode::Revoked, Vec::new());
            pipeline.fail(&revoked());
            return Err(revoked());
        };
        let size = capnp::MessageSize {
            word_count: prepared.scratch.bytes() / 8,
            cap_count: mapped.len() as u32,
        };
        let mut request = inner.new_call(
            prepared.call.interface_id,
            prepared.call.method_id,
            Some(size),
        );
        let written = prepared
            .scratch
            .reader_with(&mapped)
            .and_then(|reader| request.hook.get().set_as(reader));
        drop(mapped);
        if let Err(error) = written {
            result.record(ResultCode::Failed, Vec::new());
            pipeline.fail(&error);
            return Err(error);
        }
        let remote = request.hook.send();
        let mut paths = Vec::with_capacity(pipeline.paths.len());
        for path in pipeline.paths {
            let hook = remote
                .pipeline
                .hook
                .get_pipelined_cap(&path_operations(&path.pointers));
            self.memoize(&path.capability, hook.get_ptr());
            if path.resolver.send(Ok(hook)).is_err() {
                tracing::debug!(
                    cap_id = path.capability.cap_id,
                    "a pipelined capability was released before its call was forwarded"
                );
            }
            paths.push(FilledPath {
                pointers: path.pointers,
                capability: path.capability,
                wrapper: path.wrapper,
            });
        }
        Ok(InFlight {
            prepared,
            result,
            promise: remote.promise,
            paths,
        })
    }

    async fn finish(
        self: &Rc<Self>,
        in_flight: InFlight,
        results: &mut Results<any_pointer::Owned>,
    ) -> Result<(), capnp::Error> {
        let InFlight {
            prepared,
            result,
            promise,
            paths,
        } = in_flight;
        let response = match select(
            promise,
            Box::pin(prepared.revocation.clone().cancelled_owned()),
        )
        .await
        {
            Either::Left((Ok(response), _)) => response,
            Either::Left((Err(error), _)) => {
                result.record(ResultCode::of_error(&error), Vec::new());
                return Err(error);
            }
            Either::Right(_) => {
                tracing::debug!(
                    principal = %self.principal,
                    session_id = %self.session_id,
                    call_id = %prepared.call.call_id,
                    action = %prepared.call.action,
                    "in-flight call revoked"
                );
                result.record(ResultCode::Revoked, Vec::new());
                return Err(revoked());
            }
        };
        let copied = response.get().and_then(Scratch::copy);
        drop(response);
        let mut scratch = match copied {
            Ok(scratch) => scratch,
            Err(error) => {
                result.record(ResultCode::Failed, Vec::new());
                return Err(error);
            }
        };
        prepared.permit.add_bytes(scratch.bytes().saturating_mul(2));
        let Some(scope) = prepared
            .target
            .scope
            .upgrade()
            .filter(|_| !self.dropped.get())
        else {
            result.record(ResultCode::Revoked, Vec::new());
            return Err(revoked());
        };
        let typed = scratch.reader().and_then(|reader| {
            sanitize(
                &prepared.bundle,
                Some(prepared.method.result_struct),
                reader,
                false,
            )
        });
        let interfaces = match typed {
            Ok(sanitized) => sanitized.interfaces,
            Err(error) => {
                result.record(ResultCode::Failed, Vec::new());
                return Err(error);
            }
        };
        let mut filled = Vec::with_capacity(paths.len());
        for path in &paths {
            let resolved = scratch
                .reader()
                .and_then(|reader| reader.get_pipelined_cap(&path_operations(&path.pointers)));
            if let Ok(resolved) = resolved {
                if self.returns_home(resolved.as_ref(), prepared.target.side.opposite()) {
                    tracing::warn!(
                        principal = %self.principal,
                        session_id = %self.session_id,
                        call_id = %prepared.call.call_id,
                        action = %prepared.call.action,
                        cap_id = path.capability.cap_id,
                        "a pipelined result carries a capability back to the side it came from; the call fails"
                    );
                    result.record(ResultCode::Failed, Vec::new());
                    return Err(capnp::Error::failed(
                        "a pipelined result cannot carry a capability back to the side it came from"
                            .to_string(),
                    ));
                }
                let pointer = self.resolve_capability(&path.capability, resolved);
                filled.push((pointer, path));
            }
        }
        let table = scratch.take_table();
        let (mapped, cap_ids) = match self.map_table(
            &scope,
            table,
            prepared.target.side.opposite(),
            &prepared.call,
            &interfaces,
            &filled,
        ) {
            Ok(mapped) => mapped,
            Err(MapFailure::Limit(exceeded)) => {
                metrics::counter!("nightfall_rate_limited_total", "limit" => exceeded.limit)
                    .increment(1);
                result.record(ResultCode::Overloaded, Vec::new());
                return Err(rate_limited(exceeded.limit));
            }
            Err(MapFailure::Refused(error)) => {
                result.record(ResultCode::Failed, Vec::new());
                return Err(error);
            }
        };
        drop(scope);
        drop(filled);
        let written = scratch.reader_with(&mapped).and_then(|reader| {
            if paths.is_empty() {
                results.get().set_as(reader)
            } else {
                let slices: Vec<&[u16]> =
                    paths.iter().map(|path| path.pointers.as_slice()).collect();
                merge_into(results.get(), reader, &slices)
            }
        });
        drop(mapped);
        if let Err(error) = written {
            result.record(ResultCode::Failed, Vec::new());
            return Err(error);
        }
        result.record(ResultCode::Ok, cap_ids);
        drop(prepared);
        Ok(())
    }

    fn new_capability(
        self: &Rc<Self>,
        scope: &Rc<Scope>,
        side: Side,
        inner: Box<dyn ClientHook>,
        interface_id: u64,
        origin: &CallInfo,
    ) -> Result<Rc<Capability>, LimitExceeded> {
        let permit = self.limits.reserve_cap(&self.session_limits)?;
        let cap_id = self.next_cap_id.get();
        self.next_cap_id.set(cap_id + 1);
        let pointer = inner.get_ptr();
        let previous = scope.hooks.borrow_mut().insert(cap_id, inner);
        drop(previous);
        let capability = Rc::new(Capability {
            cap_id,
            parent_cap_id: Some(origin.cap_id),
            pid: origin.pid,
            side,
            scope: Rc::downgrade(scope),
            connection: Rc::downgrade(self),
            memo_pointer: Cell::new(pointer),
            gate: Rc::default(),
            _permit: permit,
        });
        self.memo
            .borrow_mut()
            .insert(pointer, Rc::downgrade(&capability));
        self.provenance.borrow_mut().add(ProvenanceEntry {
            cap_id,
            parent_cap_id: Some(origin.cap_id),
            call_id: Some(origin.call_id.clone()),
            action: origin.action.clone(),
            interface_id,
            principal: self.principal.clone(),
            pid: origin.pid,
            live: true,
        });
        Ok(capability)
    }

    fn mint(self: &Rc<Self>, capability: &Rc<Capability>) -> Box<dyn ClientHook> {
        let alive = Rc::new(());
        let registered = Registration {
            alive: Rc::downgrade(&alive),
            capability: Rc::downgrade(capability),
        };
        let client: WrapperClient = capnp_rpc::new_client(Wrapper {
            capability: capability.clone(),
            _alive: alive,
        });
        let hook = client.hook;
        self.wrappers
            .borrow_mut()
            .insert(hook.get_ptr(), registered);
        hook
    }

    fn resolve_capability(
        &self,
        capability: &Rc<Capability>,
        mut resolved: Box<dyn ClientHook>,
    ) -> usize {
        while let Some(further) = resolved.get_resolved() {
            resolved = further;
        }
        let pointer = resolved.get_ptr();
        let Some(scope) = capability.scope.upgrade() else {
            return pointer;
        };
        let previous = scope.hooks.borrow_mut().insert(capability.cap_id, resolved);
        drop(previous);
        self.memoize(capability, pointer);
        pointer
    }

    fn memoize(&self, capability: &Rc<Capability>, pointer: usize) {
        let old = capability.memo_pointer.replace(pointer);
        let mut memo = self.memo.borrow_mut();
        if memo
            .get(&old)
            .is_some_and(|existing| existing.as_ptr() == Rc::as_ptr(capability))
        {
            memo.remove(&old);
        }
        memo.insert(pointer, Rc::downgrade(capability));
    }

    fn map_table(
        self: &Rc<Self>,
        scope: &Rc<Scope>,
        table: CapTable,
        destination: Side,
        origin: &CallInfo,
        interfaces: &HashMap<usize, u64>,
        filled: &[(usize, &FilledPath)],
    ) -> Result<(CapTable, Vec<u64>), MapFailure> {
        if table.len() > MAXIMUM_CAPABILITIES_PER_MESSAGE {
            return Err(MapFailure::Limit(LimitExceeded {
                limit: "max_capabilities_per_message",
            }));
        }
        let mut mapped = Vec::with_capacity(table.len());
        let mut cap_ids = Vec::new();
        for entry in table {
            let Some(mut hook) = entry else {
                mapped.push(None);
                continue;
            };
            while let Some(resolved) = hook.get_resolved() {
                hook = resolved;
            }
            let pointer = hook.get_ptr();
            if let Some((_, path)) = filled.iter().find(|(filled, _)| *filled == pointer) {
                cap_ids.push(path.capability.cap_id);
                mapped.push(Some(path.wrapper.add_ref()));
                continue;
            }
            if pointer == self.bootstrap_pointer.get() {
                if destination == Side::Node {
                    return Err(MapFailure::Refused(capnp::Error::failed(
                        "the nightfall bootstrap cannot be passed to a node".to_string(),
                    )));
                }
                cap_ids.push(BOOTSTRAP_CAP_ID);
                mapped.push(Some(hook));
                continue;
            }
            let known = self
                .wrappers
                .borrow()
                .get(&pointer)
                .filter(|registration| registration.alive.strong_count() > 0)
                .and_then(|registration| registration.capability.upgrade());
            if let Some(capability) = known {
                cap_ids.push(capability.cap_id);
                if capability.side == destination {
                    let inner = capability.scope.upgrade().and_then(|owner| {
                        owner
                            .hooks
                            .borrow()
                            .get(&capability.cap_id)
                            .map(|inner| inner.add_ref())
                    });
                    mapped.push(Some(inner.unwrap_or(hook)));
                } else {
                    mapped.push(Some(hook));
                }
                continue;
            }
            let memoized = self.memo.borrow().get(&pointer).and_then(Weak::upgrade);
            let capability = match memoized {
                Some(capability) => capability,
                None => self
                    .new_capability(
                        scope,
                        destination.opposite(),
                        hook,
                        interfaces.get(&pointer).copied().unwrap_or(0),
                        origin,
                    )
                    .map_err(MapFailure::Limit)?,
            };
            let minted = self.mint(&capability);
            cap_ids.push(capability.cap_id);
            mapped.push(Some(minted));
        }
        Ok((mapped, cap_ids))
    }

    fn release(&self, reason: Option<&str>, by: Option<&str>) {
        if self.dropped.replace(true) {
            return;
        }
        if let Some(reason) = reason {
            let mut event = self.event_entry(AuditEvent::MembraneDropped, NO_PROCESS);
            let mut detail = serde_json::Map::new();
            detail.insert(
                "reason".to_string(),
                serde_json::Value::String(reason.to_string()),
            );
            if let Some(by) = by {
                detail.insert("by".to_string(), serde_json::Value::String(by.to_string()));
            }
            event.event_detail = Some(detail);
            self.record_event(event);
            metrics::counter!("nightfall_membranes_dropped_total", "reason" => reason.to_string())
                .increment(1);
        }
        let identity = self.node.identity();
        tracing::info!(
            principal = %self.principal,
            session_id = %self.session_id,
            device_id = %identity.device_id,
            installation_id = %identity.installation_id,
            namespace_id = format!("{:016x}", identity.namespace_id),
            epoch = self.node.epoch(),
            reason = reason.unwrap_or("client_closed"),
            by,
            "membrane closed"
        );
        self.revocation.cancel();
        let root = self.root.borrow_mut().take();
        drop(root);
    }
}

pub struct Membrane {
    state: Rc<ConnectionState>,
    bootstrap: Box<dyn ClientHook>,
}

impl Membrane {
    pub fn new(
        node: Rc<dyn NodeLink>,
        principal: String,
        policy: Policy,
        bundle: Arc<Bundle>,
        audit: Rc<dyn AuditSink>,
        param_key: Rc<[u8]>,
        limits: Rc<LimitState>,
    ) -> Rc<Membrane> {
        let identity = node.identity().clone();
        let epoch = node.epoch();
        let session_limits = limits.session(identity.namespace_id, epoch);
        let bucket = limits.principal_bucket(&principal, identity.namespace_id, epoch);
        let revocation = CancellationToken::new();
        let root = Rc::new(Scope {
            hooks: RefCell::new(HashMap::new()),
            revocation: revocation.child_token(),
        });
        let session_id = uuid::Uuid::now_v7().to_string();
        let refusals = RefCell::new(TokenBucket::new(
            limits.limits().calls_per_second_per_principal_per_node,
        ));
        let mut provenance =
            Provenance::bounded(limits.limits().max_live_caps_per_session.saturating_mul(2));
        provenance.add(ProvenanceEntry {
            cap_id: BOOTSTRAP_CAP_ID,
            parent_cap_id: None,
            call_id: None,
            action: "bootstrap".to_string(),
            interface_id: dusk_interface(),
            principal: principal.clone(),
            pid: NO_PROCESS,
            live: true,
        });
        let state = Rc::new(ConnectionState {
            node,
            principal,
            policy,
            bundle,
            audit,
            param_key,
            limits,
            session_limits,
            bucket,
            refusals,
            session_id,
            next_cap_id: Cell::new(BOOTSTRAP_CAP_ID + 1),
            root: RefCell::new(Some(root)),
            wrappers: RefCell::new(HashMap::new()),
            memo: RefCell::new(HashMap::new()),
            provenance: RefCell::new(provenance),
            revocation,
            dropped: Cell::new(false),
            bootstrap_pointer: Cell::new(0),
            bootstrap_gate: Rc::default(),
        });
        let client: BootstrapClient = capnp_rpc::new_client(Bootstrap {
            connection: Rc::downgrade(&state),
        });
        state.bootstrap_pointer.set(client.hook.get_ptr());
        tracing::info!(
            principal = %state.principal,
            session_id = %state.session_id,
            device_id = %identity.device_id,
            installation_id = %identity.installation_id,
            namespace_id = format!("{:016x}", identity.namespace_id),
            epoch,
            bundle = %state.bundle.name(),
            quarantined = state.policy.quarantined(),
            "membrane opened"
        );
        Rc::new(Membrane {
            state,
            bootstrap: client.hook,
        })
    }

    pub fn bootstrap(&self) -> capnp::capability::Client {
        capnp::capability::Client::new(self.bootstrap.add_ref())
    }

    pub fn drop_membrane(&self, reason: &str) {
        self.state.release(Some(reason), None);
    }

    pub fn drop_membrane_by(&self, reason: &str, by: &str) {
        self.state.release(Some(reason), Some(by));
    }

    pub fn reapply(&self, permissions: &Permissions) -> bool {
        let quarantined = self.state.policy.quarantined();
        match permissions.policy_for(&self.state.principal, quarantined) {
            Some(policy) if policy == self.state.policy => false,
            Some(_) => {
                self.drop_membrane("permissions_changed");
                true
            }
            None => {
                self.drop_membrane("principal_revoked");
                true
            }
        }
    }

    pub fn provenance(&self) -> Vec<ProvenanceEntry> {
        self.state.provenance.borrow().snapshot()
    }

    pub fn principal(&self) -> &str {
        &self.state.principal
    }

    pub fn policy(&self) -> &Policy {
        &self.state.policy
    }

    pub fn session_id(&self) -> &str {
        &self.state.session_id
    }

    pub fn dropped(&self) -> bool {
        self.state.dropped.get()
    }

    pub fn revocation(&self) -> CancellationToken {
        self.state.revocation.clone()
    }
}

impl Drop for Membrane {
    fn drop(&mut self) {
        self.state.release(None, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::{InstanceLimits, Limits};
    use crate::node::SessionIdentity;
    use crate::schema::SchemaRegistry;
    use crate::test_support::{MemoryAuditSink, TestNodeLink};

    const PERMISSIONS: &str = "[[role]]\nname = \"reader\"\nallow = [\"Dusk.ps\"]\n[[principal]]\nname = \"dawn-*\"\nroles = [\"reader\"]\n";

    fn membrane(permissions: &Permissions, principal: &str) -> (Rc<Membrane>, Rc<MemoryAuditSink>) {
        let unreachable: dusk::Client = capnp_rpc::new_future_client(async {
            Err(capnp::Error::disconnected(
                "no node in this test".to_string(),
            ))
        });
        let node = TestNodeLink::new(
            unreachable,
            SessionIdentity {
                device_id: "00112233445566778899aabbccddeeff".to_string(),
                installation_id: "ffeeddccbbaa99887766554433221100".to_string(),
                namespace_id: 7,
                instance: "nightfall-test".to_string(),
                quarantined: false,
            },
            1,
        );
        let registry =
            SchemaRegistry::load_directory(std::path::Path::new(env!("NIGHTFALL_TREE_SCHEMAS")))
                .unwrap();
        let audit = MemoryAuditSink::new();
        let limits = Limits::default();
        let instance = InstanceLimits::new(&limits);
        let membrane = Membrane::new(
            node,
            principal.to_string(),
            permissions.policy_for(principal, false).unwrap(),
            registry.bundle_for(&[]),
            audit.clone(),
            Rc::from(&b"key"[..]),
            LimitState::new(limits, instance),
        );
        (membrane, audit)
    }

    #[test]
    fn a_message_with_more_capabilities_than_the_bound_is_refused() {
        let permissions = Permissions::from_toml(PERMISSIONS).unwrap();
        let (membrane, _) = membrane(&permissions, "dawn-0");
        let state = &membrane.state;
        let scope = state.root.borrow().clone().unwrap();
        let origin = CallInfo {
            call_id: uuid::Uuid::now_v7().to_string(),
            cap_id: BOOTSTRAP_CAP_ID,
            parent_cap_id: None,
            direction: Direction::ClientToNode,
            action: "Dusk.ps".to_string(),
            interface_id: dusk_interface(),
            method_id: 2,
            pid: NO_PROCESS,
            started: Instant::now(),
        };
        let table = |count: usize| -> CapTable {
            (0..count)
                .map(|_| Some(membrane.bootstrap.add_ref()))
                .collect()
        };
        let largest = state.map_table(
            &scope,
            table(MAXIMUM_CAPABILITIES_PER_MESSAGE),
            Side::Client,
            &origin,
            &HashMap::new(),
            &[],
        );
        assert!(
            matches!(largest, Ok((_, cap_ids)) if cap_ids.len() == MAXIMUM_CAPABILITIES_PER_MESSAGE)
        );
        let refused = state.map_table(
            &scope,
            table(MAXIMUM_CAPABILITIES_PER_MESSAGE + 1),
            Side::Client,
            &origin,
            &HashMap::new(),
            &[],
        );
        assert!(matches!(
            refused,
            Err(MapFailure::Limit(LimitExceeded {
                limit: "max_capabilities_per_message"
            }))
        ));
    }

    #[test]
    fn a_reload_drops_only_the_membranes_whose_policy_changed() {
        let original = Permissions::from_toml(PERMISSIONS).unwrap();
        let (unchanged, _) = membrane(&original, "dawn-0");
        assert!(!unchanged.reapply(&Permissions::from_toml(PERMISSIONS).unwrap()));
        assert!(!unchanged.dropped());

        let (widened, audit) = membrane(&original, "dawn-1");
        let wider = PERMISSIONS.replace("\"Dusk.ps\"", "\"Dusk.ps\", \"Dusk.kill\"");
        assert!(widened.reapply(&Permissions::from_toml(&wider).unwrap()));
        assert!(widened.dropped());
        let dropped = audit.entries();
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].event, Some(AuditEvent::MembraneDropped));
        assert_eq!(
            dropped[0].event_detail.as_ref().unwrap()["reason"],
            "permissions_changed"
        );

        let (revoked, audit) = membrane(&original, "dawn-2");
        let denied = format!("deny_principals = [\"dawn-2\"]\n{PERMISSIONS}");
        assert!(revoked.reapply(&Permissions::from_toml(&denied).unwrap()));
        assert_eq!(
            audit.entries()[0].event_detail.as_ref().unwrap()["reason"],
            "principal_revoked"
        );
    }
}
