use crate::gate::{Gate, GateTicket};
use serde::Deserialize;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CidrLimit {
    pub cidr: String,
    pub handshake_failures_per_minute: Option<u32>,
    pub enrollments_per_hour: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub handshakes_per_second: u32,
    pub handshake_failures_per_ip_per_minute: u32,
    pub credential_failures_per_ip_per_hour: u32,
    pub penalty_seconds: u64,
    pub session_setups_per_identity_per_5s: u32,
    pub enrollments_per_second: u32,
    pub enrollments_per_second_per_credential: u32,
    pub enrollment_alert_per_minute: u32,
    pub calls_per_second_per_principal_per_node: u32,
    pub calls_per_second_instance: u32,
    pub reverse_calls_per_second_per_node: u32,
    pub max_inflight_calls_per_session: u32,
    pub max_inflight_reverse_calls_per_session: u32,
    pub max_waiting_calls_per_session: u32,
    pub max_message_bytes: u64,
    pub max_inflight_bytes_per_session: u64,
    pub max_inflight_bytes_instance: u64,
    pub max_live_caps_per_session: u64,
    pub max_live_caps_instance: u64,
    pub max_relayed_connections: u32,
    pub max_provisioning_connections: u32,
    pub max_provisioning_connections_per_ip: u32,
    pub write_ahead_timeout_ms: u64,
    pub stream_wait_timeout_ms: u64,
    pub cidr: Vec<CidrLimit>,
    pub per_ip_exempt_cidrs: Vec<String>,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            handshakes_per_second: 2000,
            handshake_failures_per_ip_per_minute: 30,
            credential_failures_per_ip_per_hour: 20,
            penalty_seconds: 60,
            session_setups_per_identity_per_5s: 1,
            enrollments_per_second: 50,
            enrollments_per_second_per_credential: 10,
            enrollment_alert_per_minute: 600,
            calls_per_second_per_principal_per_node: 200,
            calls_per_second_instance: 200_000,
            reverse_calls_per_second_per_node: 10_000,
            max_inflight_calls_per_session: 256,
            max_inflight_reverse_calls_per_session: 256,
            max_waiting_calls_per_session: 4096,
            max_message_bytes: 4 * 1024 * 1024,
            max_inflight_bytes_per_session: 8 * 1024 * 1024,
            max_inflight_bytes_instance: 4 * 1024 * 1024 * 1024,
            max_live_caps_per_session: 10_000,
            max_live_caps_instance: 10_000_000,
            max_relayed_connections: 10_000,
            max_provisioning_connections: 10_000,
            max_provisioning_connections_per_ip: 16,
            write_ahead_timeout_ms: 5_000,
            stream_wait_timeout_ms: 60_000,
            cidr: Vec::new(),
            per_ip_exempt_cidrs: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct TokenBucket {
    capacity: f64,
    rate: f64,
    tokens: f64,
    updated: Instant,
}

impl TokenBucket {
    pub fn new(per_second: u32) -> TokenBucket {
        let rate = f64::from(per_second.max(1));
        TokenBucket {
            capacity: rate,
            rate,
            tokens: rate,
            updated: Instant::now(),
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate).min(self.capacity);
        self.updated = now;
    }

    pub fn try_take(&mut self, now: Instant) -> bool {
        self.refill(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    pub fn refund(&mut self) {
        self.tokens = (self.tokens + 1.0).min(self.capacity);
    }

    pub fn time_until_token(&mut self, now: Instant) -> Duration {
        self.refill(now);
        if self.tokens >= 1.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64((1.0 - self.tokens) / self.rate)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LimitExceeded {
    pub limit: &'static str,
}

impl fmt::Display for LimitExceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "limit {} reached", self.limit)
    }
}

impl std::error::Error for LimitExceeded {}

pub struct InstanceLimits {
    calls: Mutex<TokenBucket>,
    inflight_bytes: AtomicU64,
    live_caps: AtomicU64,
}

impl InstanceLimits {
    pub fn new(limits: &Limits) -> Arc<InstanceLimits> {
        Arc::new(InstanceLimits {
            calls: Mutex::new(TokenBucket::new(limits.calls_per_second_instance)),
            inflight_bytes: AtomicU64::new(0),
            live_caps: AtomicU64::new(0),
        })
    }

    pub fn inflight_bytes(&self) -> u64 {
        self.inflight_bytes.load(Ordering::Relaxed)
    }

    pub fn live_caps(&self) -> u64 {
        self.live_caps.load(Ordering::Relaxed)
    }

    fn add_bytes(&self, bytes: u64) {
        let total = self.inflight_bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
        metrics::gauge!("nightfall_inflight_bytes").set(total as f64);
    }

    fn remove_bytes(&self, bytes: u64) {
        let total = self.inflight_bytes.fetch_sub(bytes, Ordering::Relaxed) - bytes;
        metrics::gauge!("nightfall_inflight_bytes").set(total as f64);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Forward,
    Reverse,
}

impl Lane {
    fn slot_limit(self) -> &'static str {
        match self {
            Lane::Forward => "max_inflight_calls_per_session",
            Lane::Reverse => "max_inflight_reverse_calls_per_session",
        }
    }

    fn rate_limit(self) -> &'static str {
        match self {
            Lane::Forward => "calls_per_second_per_principal_per_node",
            Lane::Reverse => "reverse_calls_per_second_per_node",
        }
    }
}

pub struct SessionLimits {
    forward: Rc<Gate>,
    reverse: Rc<Gate>,
    reverse_bucket: RefCell<TokenBucket>,
    waiting_calls: Cell<u32>,
    inflight_bytes: Cell<u64>,
    live_caps: Cell<u64>,
}

impl SessionLimits {
    fn new(limits: &Limits) -> SessionLimits {
        SessionLimits {
            forward: Rc::new(Gate::new(limits.max_inflight_calls_per_session as usize)),
            reverse: Rc::new(Gate::new(
                limits.max_inflight_reverse_calls_per_session as usize,
            )),
            reverse_bucket: RefCell::new(TokenBucket::new(
                limits.reverse_calls_per_second_per_node,
            )),
            waiting_calls: Cell::new(0),
            inflight_bytes: Cell::new(0),
            live_caps: Cell::new(0),
        }
    }

    fn gate(&self, lane: Lane) -> &Rc<Gate> {
        match lane {
            Lane::Forward => &self.forward,
            Lane::Reverse => &self.reverse,
        }
    }

    pub fn reverse_bucket(&self) -> &RefCell<TokenBucket> {
        &self.reverse_bucket
    }

    pub fn inflight_calls(&self, lane: Lane) -> u32 {
        self.gate(lane).used() as u32
    }

    pub fn waiting_calls(&self) -> u32 {
        self.waiting_calls.get()
    }

    pub fn inflight_bytes(&self) -> u64 {
        self.inflight_bytes.get()
    }

    pub fn live_caps(&self) -> u64 {
        self.live_caps.get()
    }
}

type PrincipalKey = (String, u64, u64);

pub struct LimitState {
    limits: Limits,
    instance: Arc<InstanceLimits>,
    principal_buckets: RefCell<HashMap<PrincipalKey, Weak<RefCell<TokenBucket>>>>,
    sessions: RefCell<HashMap<(u64, u64), Weak<SessionLimits>>>,
    bucket_prune_at: Cell<usize>,
    session_prune_at: Cell<usize>,
}

const MINIMUM_PRUNE: usize = 1024;
const LONGEST_TOKEN_WAIT: Duration = Duration::from_secs(1);

fn prune<Key, Value>(map: &mut HashMap<Key, Weak<Value>>, prune_at: &Cell<usize>) {
    if map.len() < prune_at.get() {
        return;
    }
    map.retain(|_, value| value.strong_count() > 0);
    prune_at.set((map.len() * 2).max(MINIMUM_PRUNE));
}

pub struct CallPermit {
    instance: Arc<InstanceLimits>,
    session: Rc<SessionLimits>,
    bytes: Cell<u64>,
    slot: RefCell<Option<GateTicket>>,
}

impl CallPermit {
    pub fn add_bytes(&self, bytes: u64) {
        self.bytes.set(self.bytes.get() + bytes);
        self.session
            .inflight_bytes
            .set(self.session.inflight_bytes.get() + bytes);
        self.instance.add_bytes(bytes);
    }

    pub fn holds_slot(&self) -> bool {
        self.slot.borrow().is_some()
    }

    fn take_slot(&self, ticket: GateTicket) {
        self.session
            .waiting_calls
            .set(self.session.waiting_calls.get().saturating_sub(1));
        *self.slot.borrow_mut() = Some(ticket);
    }
}

impl Drop for CallPermit {
    fn drop(&mut self) {
        if self.slot.get_mut().take().is_none() {
            self.session
                .waiting_calls
                .set(self.session.waiting_calls.get().saturating_sub(1));
        }
        let bytes = self.bytes.get();
        self.session
            .inflight_bytes
            .set(self.session.inflight_bytes.get().saturating_sub(bytes));
        self.instance.remove_bytes(bytes);
    }
}

pub struct CapPermit {
    instance: Arc<InstanceLimits>,
    session: Rc<SessionLimits>,
}

impl Drop for CapPermit {
    fn drop(&mut self) {
        self.session
            .live_caps
            .set(self.session.live_caps.get().saturating_sub(1));
        self.instance.live_caps.fetch_sub(1, Ordering::Relaxed);
    }
}

impl LimitState {
    pub fn new(limits: Limits, instance: Arc<InstanceLimits>) -> Rc<LimitState> {
        Rc::new(LimitState {
            limits,
            instance,
            principal_buckets: RefCell::new(HashMap::new()),
            sessions: RefCell::new(HashMap::new()),
            bucket_prune_at: Cell::new(MINIMUM_PRUNE),
            session_prune_at: Cell::new(MINIMUM_PRUNE),
        })
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    pub fn instance(&self) -> &Arc<InstanceLimits> {
        &self.instance
    }

    pub fn principal_bucket(
        &self,
        principal: &str,
        namespace_id: u64,
        epoch: u64,
    ) -> Rc<RefCell<TokenBucket>> {
        let mut buckets = self.principal_buckets.borrow_mut();
        prune(&mut buckets, &self.bucket_prune_at);
        let key = (principal.to_string(), namespace_id, epoch);
        if let Some(bucket) = buckets.get(&key).and_then(Weak::upgrade) {
            return bucket;
        }
        let bucket = Rc::new(RefCell::new(TokenBucket::new(
            self.limits.calls_per_second_per_principal_per_node,
        )));
        buckets.insert(key, Rc::downgrade(&bucket));
        bucket
    }

    pub fn session(&self, namespace_id: u64, epoch: u64) -> Rc<SessionLimits> {
        let mut sessions = self.sessions.borrow_mut();
        prune(&mut sessions, &self.session_prune_at);
        if let Some(session) = sessions.get(&(namespace_id, epoch)).and_then(Weak::upgrade) {
            return session;
        }
        let session = Rc::new(SessionLimits::new(&self.limits));
        sessions.insert((namespace_id, epoch), Rc::downgrade(&session));
        session
    }

    pub fn try_token(
        &self,
        bucket: &RefCell<TokenBucket>,
        lane: Lane,
    ) -> Result<(), LimitExceeded> {
        let now = Instant::now();
        if !bucket.borrow_mut().try_take(now) {
            return Err(LimitExceeded {
                limit: lane.rate_limit(),
            });
        }
        let taken = match self.instance.calls.lock() {
            Ok(mut instance_bucket) => instance_bucket.try_take(now),
            Err(poisoned) => poisoned.into_inner().try_take(now),
        };
        if !taken {
            bucket.borrow_mut().refund();
            return Err(LimitExceeded {
                limit: "calls_per_second_instance",
            });
        }
        Ok(())
    }

    pub async fn token(
        &self,
        bucket: &RefCell<TokenBucket>,
        lane: Lane,
        deadline: Instant,
    ) -> Result<(), LimitExceeded> {
        loop {
            let failure = match self.try_token(bucket, lane) {
                Ok(()) => return Ok(()),
                Err(failure) => failure,
            };
            let now = Instant::now();
            if now >= deadline {
                return Err(failure);
            }
            let instance_wait = match self.instance.calls.lock() {
                Ok(mut instance_bucket) => instance_bucket.time_until_token(now),
                Err(poisoned) => poisoned.into_inner().time_until_token(now),
            };
            let wait = bucket
                .borrow_mut()
                .time_until_token(now)
                .max(instance_wait)
                .clamp(Duration::from_millis(1), LONGEST_TOKEN_WAIT)
                .min(deadline - now);
            tokio::time::sleep(wait).await;
        }
    }

    pub fn hold(
        &self,
        session: &Rc<SessionLimits>,
        bytes: u64,
    ) -> Result<CallPermit, LimitExceeded> {
        if session.waiting_calls.get() >= self.limits.max_waiting_calls_per_session {
            return Err(LimitExceeded {
                limit: "max_waiting_calls_per_session",
            });
        }
        if session.inflight_bytes.get() + bytes > self.limits.max_inflight_bytes_per_session {
            return Err(LimitExceeded {
                limit: "max_inflight_bytes_per_session",
            });
        }
        if self.instance.inflight_bytes() + bytes > self.limits.max_inflight_bytes_instance {
            return Err(LimitExceeded {
                limit: "max_inflight_bytes_instance",
            });
        }
        session.waiting_calls.set(session.waiting_calls.get() + 1);
        let permit = CallPermit {
            instance: self.instance.clone(),
            session: session.clone(),
            bytes: Cell::new(0),
            slot: RefCell::new(None),
        };
        permit.add_bytes(bytes);
        Ok(permit)
    }

    pub fn try_slot(&self, permit: &CallPermit, lane: Lane) -> Result<(), LimitExceeded> {
        if permit.holds_slot() {
            return Ok(());
        }
        match permit.session.gate(lane).try_enter() {
            Some(ticket) => {
                permit.take_slot(ticket);
                Ok(())
            }
            None => Err(LimitExceeded {
                limit: lane.slot_limit(),
            }),
        }
    }

    pub async fn slot(
        &self,
        permit: &CallPermit,
        lane: Lane,
        deadline: Instant,
    ) -> Result<(), LimitExceeded> {
        if self.try_slot(permit, lane).is_ok() {
            return Ok(());
        }
        let gate = permit.session.gate(lane).clone();
        match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), gate.enter()).await
        {
            Ok(ticket) => {
                permit.take_slot(ticket);
                Ok(())
            }
            Err(_) => Err(LimitExceeded {
                limit: lane.slot_limit(),
            }),
        }
    }

    pub fn reserve_cap(&self, session: &Rc<SessionLimits>) -> Result<CapPermit, LimitExceeded> {
        if session.live_caps.get() >= self.limits.max_live_caps_per_session {
            return Err(LimitExceeded {
                limit: "max_live_caps_per_session",
            });
        }
        let previous = self.instance.live_caps.fetch_add(1, Ordering::Relaxed);
        if previous >= self.limits.max_live_caps_instance {
            self.instance.live_caps.fetch_sub(1, Ordering::Relaxed);
            return Err(LimitExceeded {
                limit: "max_live_caps_instance",
            });
        }
        session.live_caps.set(session.live_caps.get() + 1);
        Ok(CapPermit {
            instance: self.instance.clone(),
            session: session.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(limits: Limits) -> Rc<LimitState> {
        let instance = InstanceLimits::new(&limits);
        LimitState::new(limits, instance)
    }

    #[test]
    fn a_bucket_holds_one_second_of_tokens_and_refills_with_time() {
        let start = Instant::now();
        let mut bucket = TokenBucket::new(4);
        for _ in 0..4 {
            assert!(bucket.try_take(start));
        }
        assert!(!bucket.try_take(start));
        assert_eq!(bucket.time_until_token(start), Duration::from_millis(250));
        assert!(bucket.try_take(start + Duration::from_millis(250)));
        assert!(!bucket.try_take(start + Duration::from_millis(250)));
        bucket.refund();
        assert!(bucket.try_take(start + Duration::from_millis(250)));
        let later = start + Duration::from_secs(60);
        for _ in 0..4 {
            assert!(bucket.try_take(later));
        }
        assert!(!bucket.try_take(later));
    }

    #[test]
    fn a_bucket_of_zero_still_admits_one_call_per_second() {
        let start = Instant::now();
        let mut bucket = TokenBucket::new(0);
        assert!(bucket.try_take(start));
        assert!(!bucket.try_take(start));
    }

    #[test]
    fn bounds_calls_and_bytes_in_flight_per_session_and_releases_them_with_the_permit() {
        let limits = state(Limits {
            max_inflight_calls_per_session: 2,
            max_inflight_reverse_calls_per_session: 1,
            max_inflight_bytes_per_session: 1000,
            ..Limits::default()
        });
        let session = limits.session(1, 1);
        let first = limits.hold(&session, 400).unwrap();
        let second = limits.hold(&session, 400).unwrap();
        assert_eq!(session.waiting_calls(), 2);
        limits.try_slot(&first, Lane::Forward).unwrap();
        limits.try_slot(&second, Lane::Forward).unwrap();
        assert_eq!(session.waiting_calls(), 0);
        let third = limits.hold(&session, 1).unwrap();
        assert_eq!(
            limits.try_slot(&third, Lane::Forward).err().unwrap().limit,
            "max_inflight_calls_per_session"
        );
        limits.try_slot(&third, Lane::Reverse).unwrap();
        assert_eq!(session.inflight_calls(Lane::Reverse), 1);
        drop(first);
        assert_eq!(
            limits.hold(&session, 700).err().unwrap().limit,
            "max_inflight_bytes_per_session"
        );
        second.add_bytes(100);
        assert_eq!(session.inflight_bytes(), 501);
        assert_eq!(limits.instance().inflight_bytes(), 501);
        drop(second);
        drop(third);
        assert_eq!(session.inflight_calls(Lane::Forward), 0);
        assert_eq!(session.inflight_calls(Lane::Reverse), 0);
        assert_eq!(session.waiting_calls(), 0);
        assert_eq!(session.inflight_bytes(), 0);
        assert_eq!(limits.instance().inflight_bytes(), 0);
        let other = limits.session(2, 1);
        assert!(limits.hold(&other, 1000).is_ok());
    }

    #[test]
    fn bounds_the_calls_waiting_for_a_slot() {
        let limits = state(Limits {
            max_waiting_calls_per_session: 2,
            ..Limits::default()
        });
        let session = limits.session(1, 1);
        let first = limits.hold(&session, 1).unwrap();
        let _second = limits.hold(&session, 1).unwrap();
        assert_eq!(
            limits.hold(&session, 1).err().unwrap().limit,
            "max_waiting_calls_per_session"
        );
        limits.try_slot(&first, Lane::Forward).unwrap();
        assert!(limits.hold(&session, 1).is_ok());
    }

    #[test]
    fn shares_the_instance_byte_budget_across_sessions() {
        let limits = state(Limits {
            max_inflight_bytes_instance: 1000,
            ..Limits::default()
        });
        let held = limits.hold(&limits.session(1, 1), 800).unwrap();
        assert_eq!(
            limits.hold(&limits.session(2, 1), 300).err().unwrap().limit,
            "max_inflight_bytes_instance"
        );
        drop(held);
        assert!(limits.hold(&limits.session(2, 1), 300).is_ok());
    }

    #[test]
    fn rate_limits_per_principal_and_node_and_refunds_on_an_instance_refusal() {
        let limits = state(Limits {
            calls_per_second_per_principal_per_node: 2,
            calls_per_second_instance: 3,
            ..Limits::default()
        });
        let dawn = limits.principal_bucket("dawn-0", 1, 1);
        assert!(Rc::ptr_eq(&dawn, &limits.principal_bucket("dawn-0", 1, 1)));
        let other = limits.principal_bucket("dawn-1", 1, 1);
        assert!(limits.try_token(&dawn, Lane::Forward).is_ok());
        assert!(limits.try_token(&dawn, Lane::Forward).is_ok());
        assert_eq!(
            limits.try_token(&dawn, Lane::Forward).err().unwrap().limit,
            "calls_per_second_per_principal_per_node"
        );
        assert!(limits.try_token(&other, Lane::Forward).is_ok());
        assert_eq!(
            limits.try_token(&other, Lane::Forward).err().unwrap().limit,
            "calls_per_second_instance"
        );
        assert!(other.borrow_mut().try_take(Instant::now()));
        let session = limits.session(1, 1);
        assert_eq!(
            limits
                .try_token(session.reverse_bucket(), Lane::Reverse)
                .err()
                .unwrap()
                .limit,
            "calls_per_second_instance"
        );
    }

    #[test]
    fn a_token_wait_ends_once_the_bucket_refills() {
        let limits = state(Limits {
            reverse_calls_per_second_per_node: 20,
            ..Limits::default()
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            let session = limits.session(1, 1);
            let bucket = session.reverse_bucket();
            while limits.try_token(bucket, Lane::Reverse).is_ok() {}
            let started = Instant::now();
            limits
                .token(bucket, Lane::Reverse, started + Duration::from_secs(5))
                .await
                .unwrap();
            let waited = started.elapsed();
            assert!(waited >= Duration::from_millis(30), "{waited:?}");
            assert!(waited < Duration::from_secs(1), "{waited:?}");
            while limits.try_token(bucket, Lane::Reverse).is_ok() {}
            let refused = limits
                .token(
                    bucket,
                    Lane::Reverse,
                    Instant::now() + Duration::from_millis(5),
                )
                .await;
            assert_eq!(
                refused.err().unwrap().limit,
                "reverse_calls_per_second_per_node"
            );
        });
    }

    #[test]
    fn bounds_live_capabilities_per_session_and_instance() {
        let limits = state(Limits {
            max_live_caps_per_session: 2,
            max_live_caps_instance: 3,
            ..Limits::default()
        });
        let first_session = limits.session(1, 1);
        let second_session = limits.session(2, 1);
        let first = limits.reserve_cap(&first_session).unwrap();
        let _second = limits.reserve_cap(&first_session).unwrap();
        assert_eq!(
            limits.reserve_cap(&first_session).err().unwrap().limit,
            "max_live_caps_per_session"
        );
        let _third = limits.reserve_cap(&second_session).unwrap();
        assert_eq!(
            limits.reserve_cap(&second_session).err().unwrap().limit,
            "max_live_caps_instance"
        );
        assert_eq!(limits.instance().live_caps(), 3);
        drop(first);
        assert_eq!(first_session.live_caps(), 1);
        assert!(limits.reserve_cap(&second_session).is_ok());
    }

    #[test]
    fn hands_out_one_session_record_per_node_session_while_it_is_held() {
        let limits = state(Limits::default());
        let session = limits.session(5, 9);
        assert!(Rc::ptr_eq(&session, &limits.session(5, 9)));
        assert!(!Rc::ptr_eq(&session, &limits.session(5, 10)));
    }

    #[test]
    fn a_waiting_call_gets_the_slot_released_first_in_arrival_order() {
        let limits = state(Limits {
            max_inflight_calls_per_session: 1,
            ..Limits::default()
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            let session = limits.session(1, 1);
            let held = limits.hold(&session, 1).unwrap();
            limits.try_slot(&held, Lane::Forward).unwrap();
            let first = limits.hold(&session, 1).unwrap();
            let second = limits.hold(&session, 1).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut first_wait = Box::pin(limits.slot(&first, Lane::Forward, deadline));
            let mut second_wait = Box::pin(limits.slot(&second, Lane::Forward, deadline));
            assert!(futures::poll!(first_wait.as_mut()).is_pending());
            assert!(futures::poll!(second_wait.as_mut()).is_pending());
            let late = limits.hold(&session, 1).unwrap();
            drop(held);
            assert!(limits.try_slot(&late, Lane::Forward).is_err());
            assert!(futures::poll!(second_wait.as_mut()).is_pending());
            first_wait.await.unwrap();
            assert!(first.holds_slot());
            assert_eq!(session.inflight_calls(Lane::Forward), 1);
            assert_eq!(session.waiting_calls(), 2);
            drop(first);
            second_wait.await.unwrap();
            assert!(second.holds_slot());
        });
    }

    #[test]
    fn a_waiting_call_gives_up_at_its_deadline() {
        let limits = state(Limits {
            max_inflight_calls_per_session: 1,
            ..Limits::default()
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            let session = limits.session(1, 1);
            let held = limits.hold(&session, 1).unwrap();
            limits.try_slot(&held, Lane::Forward).unwrap();
            let waiting = limits.hold(&session, 1).unwrap();
            let started = Instant::now();
            let refused = limits
                .slot(&waiting, Lane::Forward, started + Duration::from_millis(40))
                .await;
            assert_eq!(
                refused.err().unwrap().limit,
                "max_inflight_calls_per_session"
            );
            assert!(started.elapsed() >= Duration::from_millis(40));
            drop(waiting);
            drop(held);
            assert_eq!(session.inflight_calls(Lane::Forward), 0);
            assert_eq!(session.waiting_calls(), 0);
        });
    }

    #[test]
    fn reads_the_limits_section_with_defaults_for_missing_keys() {
        let limits: Limits =
            toml::from_str("max_live_caps_per_session = 5\n[[cidr]]\ncidr = \"10.0.0.0/8\"\nenrollments_per_hour = 100\n")
                .unwrap();
        assert_eq!(limits.max_live_caps_per_session, 5);
        assert_eq!(limits.calls_per_second_per_principal_per_node, 200);
        assert_eq!(limits.cidr[0].enrollments_per_hour, Some(100));
        assert!(toml::from_str::<Limits>("unknown = 1").is_err());
    }
}
