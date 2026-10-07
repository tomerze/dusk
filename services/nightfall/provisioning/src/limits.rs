use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::time::{Duration, Instant};

pub const RENEWALS_PER_DAY: usize = 4;
const DAY: Duration = Duration::from_secs(24 * 3600);

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

    pub fn admit(&mut self, credential: &str, now: Instant) -> Result<(), RateLimited> {
        self.global.refill(now);
        if !self.per_credential.contains_key(credential)
            && self.per_credential.len() >= self.capacity
        {
            self.per_credential.retain(|_, bucket| {
                bucket.refill(now);
                !bucket.full()
            });
            if self.per_credential.len() >= self.capacity {
                return Err(RateLimited::Credential);
            }
        }
        let bucket = self
            .per_credential
            .entry(String::from(credential))
            .or_insert_with(|| TokenBucket::new(self.credential_rate, now));
        bucket.refill(now);
        if bucket.tokens < 1.0 {
            return Err(RateLimited::Credential);
        }
        if self.global.tokens < 1.0 {
            return Err(RateLimited::Instance);
        }
        bucket.tokens -= 1.0;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_the_instance_and_each_credential() {
        let now = Instant::now();
        let mut buckets = EnrollmentBuckets::new(3, 2, 10, now);
        assert_eq!(buckets.admit("a", now), Ok(()));
        assert_eq!(buckets.admit("a", now), Ok(()));
        assert_eq!(buckets.admit("a", now), Err(RateLimited::Credential));
        assert_eq!(buckets.admit("b", now), Ok(()));
        assert_eq!(buckets.admit("c", now), Err(RateLimited::Instance));
        let later = now + Duration::from_millis(500);
        assert_eq!(buckets.admit("a", later), Ok(()));
        assert_eq!(buckets.admit("c", later), Err(RateLimited::Instance));
    }

    #[test]
    fn keeps_the_credential_map_bounded() {
        let now = Instant::now();
        let mut buckets = EnrollmentBuckets::new(1000, 1, 2, now);
        buckets.admit("a", now).unwrap();
        buckets.admit("b", now).unwrap();
        assert_eq!(buckets.admit("c", now), Err(RateLimited::Credential));
        let later = now + Duration::from_secs(2);
        assert_eq!(buckets.admit("c", later), Ok(()));
        assert!(buckets.tracked_credentials() <= 2);
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
}
