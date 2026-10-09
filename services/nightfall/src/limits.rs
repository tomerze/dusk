use anyhow::{Context, bail};
use nightfall_membrane::limits::{Limits, TokenBucket};
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const TRACKED_ADDRESSES: usize = 1 << 20;
pub const TRACKED_IDENTITIES: usize = 1 << 20;
const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(3600);
const SETUP_WINDOW: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    network: IpAddr,
    prefix: u8,
}

fn canonical(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(ipv6_address) => ipv6_address
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(ipv6_address)),
        ipv4_address => ipv4_address,
    }
}

fn masked(address: IpAddr, prefix: u8) -> IpAddr {
    match address {
        IpAddr::V4(ipv4_address) => {
            let bits = u32::from(ipv4_address);
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - u32::from(prefix))
            };
            IpAddr::V4((bits & mask).into())
        }
        IpAddr::V6(ipv6_address) => {
            let bits = u128::from(ipv6_address);
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - u32::from(prefix))
            };
            IpAddr::V6((bits & mask).into())
        }
    }
}

impl Cidr {
    pub fn parse(text: &str) -> anyhow::Result<Cidr> {
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (text, None),
        };
        let address: IpAddr = address
            .trim()
            .parse()
            .with_context(|| format!("{text:?} is not an address or CIDR"))?;
        let address = canonical(address);
        let maximum = if address.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(prefix) => prefix
                .trim()
                .parse::<u8>()
                .ok()
                .filter(|prefix| *prefix <= maximum)
                .with_context(|| format!("{text:?} has a prefix length outside 0..={maximum}"))?,
            None => maximum,
        };
        if masked(address, prefix) != address {
            bail!("{text:?} has host bits set beyond its prefix length");
        }
        Ok(Cidr {
            network: address,
            prefix,
        })
    }

    pub fn contains(&self, address: IpAddr) -> bool {
        let address = canonical(address);
        address.is_ipv4() == self.network.is_ipv4() && masked(address, self.prefix) == self.network
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Copy, Debug)]
struct Window {
    started: Instant,
    count: u32,
}

impl Window {
    fn new(now: Instant) -> Window {
        Window {
            started: now,
            count: 0,
        }
    }

    fn add(&mut self, now: Instant, length: Duration) -> u32 {
        if now.saturating_duration_since(self.started) >= length {
            self.started = now;
            self.count = 0;
        }
        self.count = self.count.saturating_add(1);
        self.count
    }
}

fn address_key(address: IpAddr) -> IpAddr {
    match canonical(address) {
        IpAddr::V6(ipv6_address) => masked(IpAddr::V6(ipv6_address), 64),
        ipv4_address => ipv4_address,
    }
}

struct Generations<Key, Value> {
    current: HashMap<Key, Value>,
    previous: HashMap<Key, Value>,
    generation_capacity: usize,
}

impl<Key: Eq + Hash, Value> Generations<Key, Value> {
    fn new(capacity: usize) -> Generations<Key, Value> {
        Generations {
            current: HashMap::new(),
            previous: HashMap::new(),
            generation_capacity: (capacity / 2).max(1),
        }
    }

    fn entry(&mut self, key: Key, fresh: impl FnOnce() -> Value) -> &mut Value {
        if !self.current.contains_key(&key) {
            let value = self.previous.remove(&key).unwrap_or_else(fresh);
            if self.current.len() >= self.generation_capacity {
                self.previous = std::mem::take(&mut self.current);
            }
            return self.current.entry(key).or_insert(value);
        }
        self.current.entry(key).or_insert_with(fresh)
    }
}

pub struct HandshakeBucket {
    bucket: Mutex<TokenBucket>,
}

impl HandshakeBucket {
    pub fn new(per_second: u32) -> HandshakeBucket {
        HandshakeBucket {
            bucket: Mutex::new(TokenBucket::new(per_second)),
        }
    }

    pub fn try_take(&self) -> bool {
        lock(&self.bucket).try_take(Instant::now())
    }
}

#[derive(Clone, Debug)]
struct CidrRule {
    cidr: Cidr,
    handshake_failures_per_minute: Option<u32>,
    enrollments_per_hour: Option<u32>,
}

struct AddressRecord {
    handshake_failures: Window,
    credential_failures: Window,
}

struct Penalties {
    until: HashMap<IpAddr, Instant>,
    expiries: VecDeque<(Instant, IpAddr)>,
    capacity: usize,
}

impl Penalties {
    fn forget(&mut self, until: Instant, key: IpAddr) {
        if self.until.get(&key) == Some(&until) {
            self.until.remove(&key);
        }
    }

    fn expire(&mut self, now: Instant) {
        while let Some(&(until, key)) = self.expiries.front() {
            if until > now {
                return;
            }
            self.expiries.pop_front();
            self.forget(until, key);
        }
    }

    fn active(&self, key: &IpAddr, now: Instant) -> bool {
        self.until.get(key).is_some_and(|until| *until > now)
    }

    fn add(&mut self, key: IpAddr, until: Instant) {
        self.until.insert(key, until);
        self.expiries.push_back((until, key));
        while self.until.len() > self.capacity {
            match self.expiries.pop_front() {
                Some((oldest, oldest_key)) => self.forget(oldest, oldest_key),
                None => return,
            }
        }
    }
}

pub struct PenaltyBox {
    handshake_failures_per_minute: u32,
    credential_failures_per_hour: u32,
    penalty: Duration,
    exempt: Vec<Cidr>,
    rules: Vec<CidrRule>,
    addresses: Mutex<Generations<IpAddr, AddressRecord>>,
    penalties: Mutex<Penalties>,
    enrollments: Mutex<Vec<Window>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Handshake,
    Credential,
}

impl PenaltyBox {
    pub fn new(limits: &Limits) -> anyhow::Result<PenaltyBox> {
        PenaltyBox::with_capacity(limits, TRACKED_ADDRESSES)
    }

    pub fn with_capacity(limits: &Limits, capacity: usize) -> anyhow::Result<PenaltyBox> {
        let exempt = limits
            .per_ip_exempt_cidrs
            .iter()
            .map(|cidr| Cidr::parse(cidr))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let rules = limits
            .cidr
            .iter()
            .map(|rule| {
                Ok(CidrRule {
                    cidr: Cidr::parse(&rule.cidr)?,
                    handshake_failures_per_minute: rule.handshake_failures_per_minute,
                    enrollments_per_hour: rule.enrollments_per_hour,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let now = Instant::now();
        Ok(PenaltyBox {
            handshake_failures_per_minute: limits.handshake_failures_per_ip_per_minute,
            credential_failures_per_hour: limits.credential_failures_per_ip_per_hour,
            penalty: Duration::from_secs(limits.penalty_seconds),
            exempt,
            enrollments: Mutex::new(vec![Window::new(now); rules.len()]),
            rules,
            addresses: Mutex::new(Generations::new(capacity)),
            penalties: Mutex::new(Penalties {
                until: HashMap::new(),
                expiries: VecDeque::new(),
                capacity: capacity.max(1),
            }),
        })
    }

    fn exempt(&self, address: IpAddr) -> bool {
        self.exempt.iter().any(|cidr| cidr.contains(address))
    }

    fn rule(&self, address: IpAddr) -> Option<(usize, &CidrRule)> {
        self.rules
            .iter()
            .enumerate()
            .filter(|(_, rule)| rule.cidr.contains(address))
            .max_by_key(|(_, rule)| rule.cidr.prefix)
    }

    pub fn is_penalized(&self, address: IpAddr) -> bool {
        let address = canonical(address);
        if self.exempt(address) {
            return false;
        }
        lock(&self.penalties).active(&address_key(address), Instant::now())
    }

    pub fn failed(&self, address: IpAddr, failure: Failure) -> bool {
        let address = canonical(address);
        if self.exempt(address) {
            return false;
        }
        let now = Instant::now();
        let (threshold, window) = match failure {
            Failure::Handshake => (
                self.rule(address)
                    .and_then(|(_, rule)| rule.handshake_failures_per_minute)
                    .unwrap_or(self.handshake_failures_per_minute),
                MINUTE,
            ),
            Failure::Credential => (self.credential_failures_per_hour, HOUR),
        };
        let key = address_key(address);
        let count = {
            let mut addresses = lock(&self.addresses);
            let record = addresses.entry(key, || AddressRecord {
                handshake_failures: Window::new(now),
                credential_failures: Window::new(now),
            });
            match failure {
                Failure::Handshake => record.handshake_failures.add(now, window),
                Failure::Credential => record.credential_failures.add(now, window),
            }
        };
        if count <= threshold {
            return false;
        }
        let mut penalties = lock(&self.penalties);
        penalties.expire(now);
        if !penalties.active(&key, now) {
            penalties.add(key, now + self.penalty);
            drop(penalties);
            tracing::warn!(
                %address,
                failure = ?failure,
                count,
                threshold,
                penalty_seconds = self.penalty.as_secs(),
                "address put in the penalty box"
            );
            return true;
        }
        false
    }

    pub fn enrollment_attempt(&self, address: IpAddr) -> bool {
        let address = canonical(address);
        let Some((index, rule)) = self.rule(address) else {
            return true;
        };
        let Some(per_hour) = rule.enrollments_per_hour else {
            return true;
        };
        let count = lock(&self.enrollments)[index].add(Instant::now(), HOUR);
        count <= per_hour
    }

    pub fn penalized_count(&self) -> usize {
        let mut penalties = lock(&self.penalties);
        penalties.expire(Instant::now());
        penalties.until.len()
    }
}

impl nightfall_provisioning::limits::PenaltyBox for PenaltyBox {
    fn penalized(&self, address: IpAddr) -> bool {
        self.is_penalized(address)
    }

    fn admit_enrollment(&self, address: IpAddr) -> bool {
        if self.enrollment_attempt(address) {
            return true;
        }
        metrics::counter!("nightfall_rate_limited_total", "limit" => "cidr_enrollments_per_hour")
            .increment(1);
        false
    }

    fn credential_failed(&self, address: IpAddr) {
        self.failed(address, Failure::Credential);
    }
}

pub struct AddressSlots {
    per_address: u32,
    exempt: Vec<Cidr>,
    held: Mutex<HashMap<IpAddr, u32>>,
}

pub struct AddressSlot {
    slots: Arc<AddressSlots>,
    key: Option<IpAddr>,
}

impl AddressSlots {
    pub fn new(per_address: u32, limits: &Limits) -> anyhow::Result<AddressSlots> {
        let exempt = limits
            .per_ip_exempt_cidrs
            .iter()
            .map(|cidr| Cidr::parse(cidr))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(AddressSlots {
            per_address: per_address.max(1),
            exempt,
            held: Mutex::new(HashMap::new()),
        })
    }

    pub fn try_take(self: &Arc<AddressSlots>, address: IpAddr) -> Option<AddressSlot> {
        if self.exempt.iter().any(|cidr| cidr.contains(address)) {
            return Some(AddressSlot {
                slots: self.clone(),
                key: None,
            });
        }
        let key = address_key(address);
        let mut held = lock(&self.held);
        let count = held.get(&key).copied().unwrap_or(0);
        if count >= self.per_address {
            return None;
        }
        held.insert(key, count + 1);
        Some(AddressSlot {
            slots: self.clone(),
            key: Some(key),
        })
    }

    pub fn held(&self) -> usize {
        lock(&self.held).len()
    }
}

impl Drop for AddressSlot {
    fn drop(&mut self) {
        let Some(key) = self.key else {
            return;
        };
        let mut held = lock(&self.slots.held);
        match held.get_mut(&key) {
            Some(count) if *count > 1 => *count -= 1,
            Some(_) => {
                held.remove(&key);
            }
            None => tracing::error!(address = %key, "a released address slot was not held"),
        }
    }
}

pub struct SetupRate {
    per_window: u32,
    identities: Mutex<Generations<(String, String), Window>>,
}

impl SetupRate {
    pub fn new(per_window: u32) -> SetupRate {
        SetupRate::with_capacity(per_window, TRACKED_IDENTITIES)
    }

    pub fn with_capacity(per_window: u32, capacity: usize) -> SetupRate {
        SetupRate {
            per_window: per_window.max(1),
            identities: Mutex::new(Generations::new(capacity)),
        }
    }

    pub fn admit(&self, device_id: &str, installation_id: &str) -> bool {
        let now = Instant::now();
        let key = (device_id.to_string(), installation_id.to_string());
        lock(&self.identities)
            .entry(key, || Window::new(now))
            .add(now, SETUP_WINDOW)
            <= self.per_window
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightfall_membrane::limits::CidrLimit;

    fn address(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn parses_and_matches_cidrs() {
        let network = Cidr::parse("10.1.0.0/16").unwrap();
        assert!(network.contains(address("10.1.200.3")));
        assert!(!network.contains(address("10.2.0.1")));
        assert!(network.contains(address("::ffff:10.1.0.9")));
        let single = Cidr::parse("192.0.2.7").unwrap();
        assert!(single.contains(address("192.0.2.7")));
        assert!(!single.contains(address("192.0.2.8")));
        let documentation = Cidr::parse("2001:db8::/32").unwrap();
        assert!(documentation.contains(address("2001:db8:1::1")));
        assert!(!documentation.contains(address("10.1.0.1")));
        assert!(
            Cidr::parse("0.0.0.0/0")
                .unwrap()
                .contains(address("203.0.113.1"))
        );
        assert!(Cidr::parse("10.0.0.0/33").is_err());
        assert!(Cidr::parse("10.0.0.1/8").is_err());
        assert!(Cidr::parse("bogus").is_err());
    }

    #[test]
    fn penalizes_an_address_past_its_handshake_failures() {
        let limits = Limits {
            handshake_failures_per_ip_per_minute: 3,
            ..Limits::default()
        };
        let penalty_box = PenaltyBox::new(&limits).unwrap();
        let peer = address("198.51.100.4");
        for _ in 0..3 {
            assert!(!penalty_box.failed(peer, Failure::Handshake));
        }
        assert!(!penalty_box.is_penalized(peer));
        assert!(penalty_box.failed(peer, Failure::Handshake));
        assert!(penalty_box.is_penalized(peer));
        assert!(!penalty_box.is_penalized(address("198.51.100.5")));
        assert_eq!(penalty_box.penalized_count(), 1);
    }

    #[test]
    fn counts_credential_failures_on_their_own_threshold() {
        let limits = Limits {
            credential_failures_per_ip_per_hour: 2,
            ..Limits::default()
        };
        let penalty_box = PenaltyBox::new(&limits).unwrap();
        let peer = address("198.51.100.9");
        use nightfall_provisioning::limits::PenaltyBox as _;
        penalty_box.credential_failed(peer);
        penalty_box.credential_failed(peer);
        assert!(!penalty_box.penalized(peer));
        penalty_box.credential_failed(peer);
        assert!(penalty_box.penalized(peer));
    }

    #[test]
    fn never_penalizes_exempt_networks_and_applies_cidr_overrides() {
        let limits = Limits {
            handshake_failures_per_ip_per_minute: 1,
            per_ip_exempt_cidrs: vec!["100.64.0.0/10".to_string()],
            cidr: vec![CidrLimit {
                cidr: "203.0.113.0/24".to_string(),
                handshake_failures_per_minute: Some(5),
                enrollments_per_hour: Some(2),
            }],
            ..Limits::default()
        };
        let penalty_box = PenaltyBox::new(&limits).unwrap();
        let nat = address("100.64.3.3");
        for _ in 0..50 {
            penalty_box.failed(nat, Failure::Handshake);
        }
        assert!(!penalty_box.is_penalized(nat));
        let office = address("203.0.113.40");
        for _ in 0..5 {
            assert!(!penalty_box.failed(office, Failure::Handshake));
        }
        assert!(penalty_box.failed(office, Failure::Handshake));
        assert!(penalty_box.enrollment_attempt(address("203.0.113.41")));
        assert!(penalty_box.enrollment_attempt(address("203.0.113.42")));
        assert!(!penalty_box.enrollment_attempt(address("203.0.113.43")));
        assert!(penalty_box.enrollment_attempt(address("198.51.100.1")));
    }

    #[test]
    fn a_full_penalty_box_forgets_its_oldest_penalty_and_keeps_counting() {
        let limits = Limits {
            handshake_failures_per_ip_per_minute: 0,
            ..Limits::default()
        };
        let penalty_box = PenaltyBox::with_capacity(&limits, 4).unwrap();
        assert!(penalty_box.failed(address("192.0.2.1"), Failure::Handshake));
        assert!(penalty_box.failed(address("192.0.2.2"), Failure::Handshake));
        assert!(penalty_box.failed(address("192.0.2.3"), Failure::Handshake));
        assert!(penalty_box.is_penalized(address("192.0.2.1")));
        assert!(!penalty_box.failed(address("192.0.2.1"), Failure::Handshake));
        assert!(penalty_box.failed(address("192.0.2.4"), Failure::Handshake));
        assert!(penalty_box.failed(address("192.0.2.5"), Failure::Handshake));
        assert_eq!(penalty_box.penalized_count(), 4);
        assert!(!penalty_box.is_penalized(address("192.0.2.1")));
        assert!(penalty_box.is_penalized(address("192.0.2.2")));
        assert!(penalty_box.is_penalized(address("192.0.2.5")));
        assert!(!penalty_box.failed(address("192.0.2.2"), Failure::Handshake));
        assert!(penalty_box.failed(address("192.0.2.6"), Failure::Handshake));
        assert_eq!(penalty_box.penalized_count(), 4);
        assert!(!penalty_box.is_penalized(address("192.0.2.2")));
        assert!(penalty_box.is_penalized(address("192.0.2.6")));
    }

    #[test]
    fn keys_ipv6_addresses_by_their_slash_64() {
        let limits = Limits {
            handshake_failures_per_ip_per_minute: 1,
            ..Limits::default()
        };
        let penalty_box = PenaltyBox::new(&limits).unwrap();
        assert!(!penalty_box.failed(address("2001:db8:1:2::1"), Failure::Handshake));
        assert!(penalty_box.failed(address("2001:db8:1:2::ffff"), Failure::Handshake));
        assert!(penalty_box.is_penalized(address("2001:db8:1:2:abcd::9")));
        assert!(!penalty_box.is_penalized(address("2001:db8:1:3::1")));
    }

    #[test]
    fn holds_at_most_the_per_address_connections_and_releases_them() {
        let limits = Limits {
            per_ip_exempt_cidrs: vec!["198.51.100.0/24".to_string()],
            ..Limits::default()
        };
        let slots = Arc::new(AddressSlots::new(2, &limits).unwrap());
        let first = slots.try_take(address("192.0.2.1")).unwrap();
        let second = slots.try_take(address("192.0.2.1")).unwrap();
        assert!(slots.try_take(address("192.0.2.1")).is_none());
        assert!(slots.try_take(address("192.0.2.2")).is_some());
        let near = slots.try_take(address("2001:db8::1")).unwrap();
        let _neighbour = slots.try_take(address("2001:db8::2")).unwrap();
        assert!(slots.try_take(address("2001:db8::3")).is_none());
        let exempt: Vec<AddressSlot> = (0..5)
            .map(|_| slots.try_take(address("198.51.100.7")).unwrap())
            .collect();
        drop(first);
        assert!(slots.try_take(address("192.0.2.1")).is_some());
        drop(second);
        drop(near);
        drop(exempt);
        assert_eq!(slots.held(), 1);
    }

    #[test]
    fn the_handshake_bucket_admits_its_rate_and_then_refuses() {
        let bucket = HandshakeBucket::new(3);
        assert!(bucket.try_take());
        assert!(bucket.try_take());
        assert!(bucket.try_take());
        assert!(!bucket.try_take());
    }

    #[test]
    fn limits_session_setups_per_identity_in_five_seconds() {
        let rate = SetupRate::new(1);
        assert!(rate.admit("aa", "bb"));
        assert!(!rate.admit("aa", "bb"));
        assert!(rate.admit("aa", "cc"));
        let bounded = SetupRate::with_capacity(1, 2);
        assert!(bounded.admit("aa", "bb"));
        assert!(bounded.admit("aa", "cc"));
        assert!(!bounded.admit("aa", "cc"));
        assert!(bounded.admit("aa", "dd"));
        assert!(bounded.admit("aa", "bb"));
    }
}
