use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::time::{Duration, Instant};

use tracing::warn;

pub const RENEWALS_PER_DAY: usize = 4;
pub const COLLISION_INSTALLATIONS: usize = 20;
pub const COLLISION_ADDRESSES: usize = 5;
const DAY: Duration = Duration::from_secs(24 * 3600);
const COLLISION_ENTRIES_PER_DEVICE: usize = 64;

pub trait PenaltyBox: Send + Sync {
    fn penalized(&self, address: IpAddr) -> bool;
    fn admit_enrollment(&self, address: IpAddr) -> bool;
    fn credential_failed(&self, address: IpAddr);
}

#[derive(Debug, Clone)]
struct TokenBucket {
    capacity: f64,
    rate: f64,
    tokens: f64,
    updated: Instant,
}

impl TokenBucket {
    fn new(per_second: u32, now: Instant) -> TokenBucket {
        let rate = f64::from(per_second);
        TokenBucket {
            capacity: rate.max(1.0),
            rate,
            tokens: rate.max(1.0),
            updated: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate).min(self.capacity);
        self.updated = now;
    }

    fn full(&self) -> bool {
        self.tokens >= self.capacity
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RateLimited {
    #[error("the instance enrollment rate is exhausted")]
    Instance,
    #[error("the enrollment rate of this credential is exhausted")]
    Credential,
    #[error("the enrollment rate of this TPM endorsement key is exhausted")]
    EndorsementKey,
}

#[derive(Debug)]
pub struct EnrollmentBuckets {
    global: TokenBucket,
    per_credential: HashMap<String, TokenBucket>,
    credential_rate: u32,
    capacity: usize,
}

impl EnrollmentBuckets {
    pub fn new(
        per_second: u32,
        per_second_per_credential: u32,
        capacity: usize,
        now: Instant,
    ) -> EnrollmentBuckets {
        EnrollmentBuckets {
            global: TokenBucket::new(per_second, now),
            per_credential: HashMap::new(),
            credential_rate: per_second_per_credential,
            capacity,
        }
    }

    pub fn admit(
        &mut self,
        credential: &str,
        endorsement_key: Option<&str>,
        now: Instant,
    ) -> Result<(), RateLimited> {
        self.global.refill(now);
        let keys = [
            Some((credential, RateLimited::Credential)),
            endorsement_key.map(|key| (key, RateLimited::EndorsementKey)),
        ];
        let missing = keys
            .iter()
            .flatten()
            .filter(|(key, _)| !self.per_credential.contains_key(*key))
            .count();
        if missing > 0 && self.per_credential.len() + missing > self.capacity {
            self.per_credential.retain(|_, bucket| {
                bucket.refill(now);
                !bucket.full()
            });
            if self.per_credential.len() + missing > self.capacity {
                return Err(RateLimited::Credential);
            }
        }
        for (key, limited) in keys.iter().flatten() {
            let bucket = self
                .per_credential
                .entry(String::from(*key))
                .or_insert_with(|| TokenBucket::new(self.credential_rate, now));
            bucket.refill(now);
            if bucket.tokens < 1.0 {
                return Err(*limited);
            }
        }
        if self.global.tokens < 1.0 {
            return Err(RateLimited::Instance);
        }
        for (key, _) in keys.iter().flatten() {
            if let Some(bucket) = self.per_credential.get_mut(*key) {
                bucket.tokens -= 1.0;
            }
        }
        self.global.tokens -= 1.0;
        Ok(())
    }

    pub fn tracked_credentials(&self) -> usize {
        self.per_credential.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("this identity renewed {RENEWALS_PER_DAY} times in the last 24 hours")]
pub struct RenewLimited;

#[derive(Debug)]
pub struct RenewLimiter {
    renewals: HashMap<(String, String), VecDeque<Instant>>,
    order: VecDeque<((String, String), Instant)>,
    capacity: usize,
}

impl RenewLimiter {
    pub fn new(capacity: usize) -> RenewLimiter {
        RenewLimiter {
            renewals: HashMap::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    fn latest(&self, key: &(String, String)) -> Option<Instant> {
        self.renewals
            .get(key)
            .and_then(|times| times.back().copied())
    }

    fn forget_expired(&mut self, now: Instant) {
        while let Some((key, time)) = self.order.front().cloned() {
            let current = self.latest(&key) == Some(time);
            if current && now.saturating_duration_since(time) < DAY {
                return;
            }
            self.order.pop_front();
            if current {
                self.renewals.remove(&key);
            }
        }
    }

    fn evict_oldest(&mut self) {
        while let Some((key, time)) = self.order.pop_front() {
            if self.latest(&key) == Some(time) {
                self.renewals.remove(&key);
                return;
            }
        }
    }

    pub fn admit(
        &mut self,
        device_id: &str,
        installation_id: &str,
        now: Instant,
    ) -> Result<(), RenewLimited> {
        self.forget_expired(now);
        let key = (String::from(device_id), String::from(installation_id));
        if !self.renewals.contains_key(&key) && self.renewals.len() >= self.capacity {
            self.evict_oldest();
        }
        let times = self.renewals.entry(key.clone()).or_default();
        times.retain(|time| now.saturating_duration_since(*time) < DAY);
        if times.len() >= RENEWALS_PER_DAY {
            return Err(RenewLimited);
        }
        times.push_back(now);
        self.order.push_back((key, now));
        if self.order.len() > self.capacity.saturating_mul(2).max(16) {
            let renewals = &self.renewals;
            self.order.retain(|(key, time)| {
                renewals.get(key).and_then(|times| times.back()) == Some(time)
            });
        }
        Ok(())
    }

    pub fn release(&mut self, device_id: &str, installation_id: &str) {
        let key = (String::from(device_id), String::from(installation_id));
        if let Some(times) = self.renewals.get_mut(&key) {
            times.pop_back();
            if let Some(latest) = times.back().copied() {
                self.order.push_back((key, latest));
            } else {
                self.renewals.remove(&key);
            }
        }
    }
}

#[derive(Debug)]
pub struct RateWindow {
    seconds: [u64; 60],
    current: u64,
    origin: Instant,
}

impl RateWindow {
    pub fn new(now: Instant) -> RateWindow {
        RateWindow {
            seconds: [0; 60],
            current: 0,
            origin: now,
        }
    }

    pub fn record(&mut self, now: Instant) -> u64 {
        self.count(now);
        self.seconds[(self.current % 60) as usize] += 1;
        self.seconds.iter().sum()
    }

    pub fn count(&mut self, now: Instant) -> u64 {
        let second = now.saturating_duration_since(self.origin).as_secs();
        if second.saturating_sub(self.current) >= 60 {
            self.seconds = [0; 60];
        } else {
            for stale in self.current + 1..=second {
                self.seconds[(stale % 60) as usize] = 0;
            }
        }
        self.current = self.current.max(second);
        self.seconds.iter().sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Collision {
    pub installations: usize,
    pub addresses: usize,
}

#[derive(Debug, Default)]
struct DeviceHistory {
    enrollments: VecDeque<(String, IpAddr, Instant)>,
    reported: Option<Instant>,
}

impl DeviceHistory {
    fn latest(&self) -> Option<Instant> {
        self.enrollments.back().map(|(_, _, time)| *time)
    }
}

#[derive(Debug)]
pub struct CollisionTracker {
    devices: HashMap<String, DeviceHistory>,
    order: VecDeque<(String, Instant)>,
    capacity: usize,
}

impl CollisionTracker {
    pub fn new(capacity: usize) -> CollisionTracker {
        CollisionTracker {
            devices: HashMap::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    fn latest(&self, device_id: &str) -> Option<Instant> {
        self.devices.get(device_id).and_then(DeviceHistory::latest)
    }

    pub fn record(
        &mut self,
        device_id: &str,
        installation_id: &str,
        address: IpAddr,
        now: Instant,
    ) -> Option<Collision> {
        while let Some((front, time)) = self.order.front().cloned() {
            let current = self.latest(&front) == Some(time);
            if current && now.saturating_duration_since(time) < DAY {
                break;
            }
            self.order.pop_front();
            if current {
                self.devices.remove(&front);
            }
        }
        if !self.devices.contains_key(device_id) && self.devices.len() >= self.capacity {
            while let Some((oldest, time)) = self.order.pop_front() {
                if self.latest(&oldest) != Some(time) {
                    continue;
                }
                self.devices.remove(&oldest);
                metrics::counter!("nightfall_collision_histories_evicted_total").increment(1);
                warn!(
                    device_id = %oldest,
                    capacity = self.capacity,
                    "the device id collision tracker is full; the oldest device's enrollments of the last 24 hours are forgotten"
                );
                break;
            }
        }
        if self.order.len() > self.capacity.saturating_mul(2).max(16) {
            let devices = &self.devices;
            self.order.retain(|(device, time)| {
                devices.get(device).and_then(DeviceHistory::latest) == Some(*time)
            });
        }
        let history = self.devices.entry(String::from(device_id)).or_default();
        history
            .enrollments
            .retain(|(_, _, time)| now.saturating_duration_since(*time) < DAY);
        if history.enrollments.len() >= COLLISION_ENTRIES_PER_DEVICE {
            history.enrollments.pop_front();
        }
        history
            .enrollments
            .push_back((String::from(installation_id), address, now));
        self.order.push_back((String::from(device_id), now));
        let installations: HashSet<&str> = history
            .enrollments
            .iter()
            .map(|(installation, _, _)| installation.as_str())
            .collect();
        let addresses: HashSet<IpAddr> = history
            .enrollments
            .iter()
            .map(|(_, address, _)| *address)
            .collect();
        let collision = Collision {
            installations: installations.len(),
            addresses: addresses.len(),
        };
        let recently_reported = history
            .reported
            .is_some_and(|reported| now.saturating_duration_since(reported) < DAY);
        if collision.installations > COLLISION_INSTALLATIONS
            && collision.addresses > COLLISION_ADDRESSES
            && !recently_reported
        {
            history.reported = Some(now);
            return Some(collision);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[test]
    fn limits_the_instance_and_each_credential() {
        let now = Instant::now();
        let mut buckets = EnrollmentBuckets::new(3, 2, 10, now);
        assert_eq!(buckets.admit("a", None, now), Ok(()));
        assert_eq!(buckets.admit("a", None, now), Ok(()));
        assert_eq!(buckets.admit("a", None, now), Err(RateLimited::Credential));
        assert_eq!(buckets.admit("b", None, now), Ok(()));
        assert_eq!(buckets.admit("c", None, now), Err(RateLimited::Instance));
        let later = now + Duration::from_millis(500);
        assert_eq!(buckets.admit("a", None, later), Ok(()));
        assert_eq!(buckets.admit("c", None, later), Err(RateLimited::Instance));
    }

    #[test]
    fn keeps_the_credential_map_bounded() {
        let now = Instant::now();
        let mut buckets = EnrollmentBuckets::new(1000, 1, 2, now);
        buckets.admit("a", None, now).unwrap();
        buckets.admit("b", None, now).unwrap();
        assert_eq!(buckets.admit("c", None, now), Err(RateLimited::Credential));
        let later = now + Duration::from_secs(2);
        assert_eq!(buckets.admit("c", None, later), Ok(()));
        assert!(buckets.tracked_credentials() <= 2);
        assert_eq!(
            buckets.admit("d", Some("e"), later),
            Err(RateLimited::Credential)
        );
    }

    #[test]
    fn limits_each_endorsement_key_beside_its_credential() {
        let now = Instant::now();
        let mut buckets = EnrollmentBuckets::new(100, 2, 10, now);
        assert_eq!(buckets.admit("token", Some("tpm:1"), now), Ok(()));
        assert_eq!(buckets.admit("other", Some("tpm:1"), now), Ok(()));
        assert_eq!(
            buckets.admit("third", Some("tpm:1"), now),
            Err(RateLimited::EndorsementKey)
        );
        assert_eq!(buckets.admit("token", Some("tpm:2"), now), Ok(()));
        assert_eq!(
            buckets.admit("token", Some("tpm:3"), now),
            Err(RateLimited::Credential)
        );
        assert_eq!(buckets.admit("third", None, now), Ok(()));
    }

    #[test]
    fn allows_four_renewals_a_day() {
        let now = Instant::now();
        let mut limiter = RenewLimiter::new(10);
        for hour in 0..4 {
            limiter
                .admit("d", "i", now + Duration::from_secs(hour * 3600))
                .unwrap();
        }
        assert_eq!(
            limiter.admit("d", "i", now + Duration::from_secs(5 * 3600)),
            Err(RenewLimited)
        );
        limiter.release("d", "i");
        assert_eq!(
            limiter.admit("d", "i", now + Duration::from_secs(5 * 3600)),
            Ok(())
        );
        assert_eq!(limiter.admit("d", "other", now), Ok(()));
        assert_eq!(limiter.admit("d", "i", now + DAY), Ok(()));
    }

    #[test]
    fn evicts_the_oldest_renewal_history_at_capacity() {
        let now = Instant::now();
        let mut limiter = RenewLimiter::new(2);
        limiter.admit("d", "a", now).unwrap();
        limiter
            .admit("d", "b", now + Duration::from_secs(1))
            .unwrap();
        limiter
            .admit("d", "a", now + Duration::from_secs(2))
            .unwrap();
        limiter
            .admit("d", "c", now + Duration::from_secs(3))
            .unwrap();
        assert_eq!(limiter.renewals.len(), 2);
        assert!(limiter.renewals.contains_key(&("d".into(), "a".into())));
        assert!(limiter.renewals.contains_key(&("d".into(), "c".into())));
        limiter
            .admit("d", "e", now + DAY + Duration::from_secs(3))
            .unwrap();
        assert_eq!(limiter.renewals.len(), 1);
        for second in 0..100 {
            limiter
                .admit("d", "e", now + DAY + Duration::from_secs(10 + second))
                .ok();
        }
        assert!(limiter.order.len() <= 16, "{}", limiter.order.len());
    }

    #[test]
    fn forgets_expired_devices_and_evicts_the_oldest_live_one_at_capacity() {
        let now = Instant::now();
        let address = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1));
        let mut tracker = CollisionTracker::new(2);
        tracker.record("a", "1", address, now);
        tracker.record("b", "1", address, now + Duration::from_secs(1));
        tracker.record("a", "2", address, now + Duration::from_secs(2));
        tracker.record("c", "1", address, now + Duration::from_secs(3));
        assert_eq!(tracker.devices.len(), 2);
        assert!(tracker.devices.contains_key("a"));
        assert!(tracker.devices.contains_key("c"));
        tracker.record("d", "1", address, now + DAY + Duration::from_secs(3));
        assert_eq!(tracker.devices.len(), 1);
        assert!(tracker.devices.contains_key("d"));
        for second in 0..100 {
            tracker.record(
                "d",
                "1",
                address,
                now + DAY + Duration::from_secs(10 + second),
            );
        }
        assert!(tracker.order.len() <= 16, "{}", tracker.order.len());
    }

    #[test]
    fn counts_the_last_minute() {
        let now = Instant::now();
        let mut window = RateWindow::new(now);
        assert_eq!(window.record(now), 1);
        assert_eq!(window.record(now + Duration::from_secs(30)), 2);
        assert_eq!(window.record(now + Duration::from_secs(59)), 3);
        assert_eq!(window.record(now + Duration::from_secs(61)), 3);
        assert_eq!(window.record(now + Duration::from_secs(200)), 1);
        assert_eq!(window.count(now + Duration::from_secs(230)), 1);
        assert_eq!(window.count(now + Duration::from_secs(261)), 0);
    }

    #[test]
    fn reports_a_cloned_image_once_a_day() {
        let now = Instant::now();
        let mut tracker = CollisionTracker::new(10);
        let mut reports = Vec::new();
        for number in 0..30u8 {
            let address = IpAddr::V4(Ipv4Addr::new(198, 51, 100, number % 8));
            if let Some(collision) =
                tracker.record("device", &format!("{number:032x}"), address, now)
            {
                reports.push((number, collision));
            }
        }
        assert_eq!(
            reports,
            vec![(
                20,
                Collision {
                    installations: 21,
                    addresses: 8
                }
            )]
        );
        let mut few_addresses = CollisionTracker::new(10);
        for number in 0..30u8 {
            let address = IpAddr::V4(Ipv4Addr::new(198, 51, 100, number % 5));
            assert!(
                few_addresses
                    .record("device", &format!("{number:032x}"), address, now)
                    .is_none()
            );
        }
    }
}
