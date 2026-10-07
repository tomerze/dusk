use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub credential_digest: [u8; 32],
    pub fingerprint_digest: [u8; 32],
    pub device_id: String,
    pub installation_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the challenge store holds {0} unexpired challenges")]
pub struct StoreFull(pub usize);

struct Entries {
    bindings: HashMap<[u8; 32], (Binding, Instant)>,
    order: VecDeque<([u8; 32], Instant)>,
}

pub struct ChallengeStore {
    entries: Mutex<Entries>,
    capacity: usize,
    ttl: Duration,
}

impl ChallengeStore {
    pub fn new(capacity: usize, ttl: Duration) -> ChallengeStore {
        ChallengeStore {
            entries: Mutex::new(Entries {
                bindings: HashMap::new(),
                order: VecDeque::new(),
            }),
            capacity,
            ttl,
        }
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    pub fn len(&self) -> usize {
        self.lock().bindings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Entries> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn issue(
        &self,
        challenge: [u8; 32],
        binding: Binding,
        now: Instant,
    ) -> Result<Instant, StoreFull> {
        let mut entries = self.lock();
        while let Some((front, expires)) = entries.order.front().copied() {
            let still_present = entries
                .bindings
                .get(&front)
                .is_some_and(|(_, stored)| *stored == expires);
            if still_present && expires > now {
                break;
            }
            entries.order.pop_front();
            if still_present {
                entries.bindings.remove(&front);
            }
        }
        if entries.bindings.len() >= self.capacity {
            return Err(StoreFull(entries.bindings.len()));
        }
        if entries.order.len() >= self.capacity.saturating_mul(2) {
            let Entries { bindings, order } = &mut *entries;
            order.retain(|(key, expires)| {
                bindings
                    .get(key)
                    .is_some_and(|(_, stored)| stored == expires)
            });
        }
        let expires = now + self.ttl;
        entries.bindings.insert(challenge, (binding, expires));
        entries.order.push_back((challenge, expires));
        Ok(expires)
    }

    pub fn take(&self, challenge: &[u8], now: Instant) -> Option<Binding> {
        let key: [u8; 32] = challenge.try_into().ok()?;
        let (binding, expires) = self.lock().bindings.remove(&key)?;
        (expires > now).then_some(binding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(number: u8) -> Binding {
        Binding {
            credential_digest: [number; 32],
            fingerprint_digest: [number; 32],
            device_id: format!("{number:032x}"),
            installation_id: format!("{:032x}", number as u32 + 1000),
        }
    }

    #[test]
    fn hands_out_a_challenge_once() {
        let store = ChallengeStore::new(4, Duration::from_secs(300));
        let now = Instant::now();
        store.issue([1; 32], binding(1), now).unwrap();
        assert_eq!(store.take(&[1; 32], now), Some(binding(1)));
        assert_eq!(store.take(&[1; 32], now), None);
        assert_eq!(store.take(&[1; 31], now), None);
        assert!(store.is_empty());
    }

    #[test]
    fn expires_challenges() {
        let store = ChallengeStore::new(4, Duration::from_secs(300));
        let now = Instant::now();
        store.issue([1; 32], binding(1), now).unwrap();
        assert_eq!(store.take(&[1; 32], now + Duration::from_secs(300)), None);
    }

    #[test]
    fn stays_bounded_and_reclaims_expired_slots() {
        let store = ChallengeStore::new(2, Duration::from_secs(300));
        let now = Instant::now();
        store.issue([1; 32], binding(1), now).unwrap();
        store
            .issue([2; 32], binding(2), now + Duration::from_secs(1))
            .unwrap();
        assert_eq!(
            store.issue([3; 32], binding(3), now + Duration::from_secs(2)),
            Err(StoreFull(2))
        );
        store.take(&[2; 32], now).unwrap();
        store
            .issue([3; 32], binding(3), now + Duration::from_secs(2))
            .unwrap();
        let later = now + Duration::from_secs(301);
        store.issue([4; 32], binding(4), later).unwrap();
        assert_eq!(store.len(), 2);
        assert_eq!(store.take(&[1; 32], later), None);
        assert_eq!(store.take(&[3; 32], later), Some(binding(3)));
    }
}
