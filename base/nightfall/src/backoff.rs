use std::prelude::rust_2024::*;

use core::time::Duration;

#[derive(Debug, Clone)]
pub(crate) struct Backoff {
    base: Duration,
    cap: Duration,
    attempt: u32,
}

impl Backoff {
    pub(crate) fn new(base: Duration, cap: Duration) -> Self {
        Backoff {
            base,
            cap,
            attempt: 0,
        }
    }

    pub(crate) fn ceiling(&self) -> Duration {
        let mut ceiling = self.base;
        for _ in 0..self.attempt {
            if ceiling >= self.cap || ceiling.is_zero() {
                break;
            }
            ceiling = ceiling.saturating_mul(2);
        }
        ceiling.min(self.cap)
    }

    pub(crate) fn next_delay(&mut self, random: u64) -> Duration {
        let ceiling = self.ceiling();
        if ceiling < self.cap && !ceiling.is_zero() {
            self.attempt = self.attempt.saturating_add(1);
        }
        uniform_up_to(ceiling, random)
    }

    pub(crate) fn at_cap(&self, random: u64) -> Duration {
        let half = self.cap / 2;
        half + uniform_up_to(self.cap - half, random)
    }

    pub(crate) fn reset(&mut self) {
        self.attempt = 0;
    }
}

pub(crate) fn uniform_up_to(ceiling: Duration, random: u64) -> Duration {
    let nanoseconds = u64::try_from(ceiling.as_nanos()).unwrap_or(u64::MAX);
    match nanoseconds.checked_add(1) {
        Some(span) => Duration::from_nanos(random % span),
        None => Duration::from_nanos(random),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn ceilings_double_from_the_base_up_to_the_cap() {
        let mut backoff = Backoff::new(SECOND, Duration::from_secs(600));
        let mut ceilings = Vec::new();
        for _ in 0..13 {
            ceilings.push(backoff.ceiling().as_secs());
            backoff.next_delay(0);
        }
        assert_eq!(
            ceilings,
            [1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 600, 600, 600]
        );
    }

    #[test]
    fn every_delay_is_at_most_its_ceiling() {
        let mut backoff = Backoff::new(SECOND, Duration::from_secs(300));
        for random in [0, 1, 7, u64::MAX / 3, u64::MAX - 1, u64::MAX] {
            backoff.reset();
            for _ in 0..20 {
                let ceiling = backoff.ceiling();
                let delay = backoff.next_delay(random);
                assert!(delay <= ceiling, "{delay:?} exceeds {ceiling:?}");
            }
        }
    }

    #[test]
    fn delays_are_spread_over_the_whole_interval() {
        let backoff = Backoff::new(SECOND, Duration::from_secs(300));
        let ceiling = u64::try_from(backoff.ceiling().as_nanos()).unwrap();
        assert_eq!(backoff.clone().next_delay(0), Duration::ZERO);
        assert_eq!(
            backoff.clone().next_delay(ceiling),
            Duration::from_nanos(ceiling)
        );
        assert_eq!(
            backoff.clone().next_delay(ceiling / 2),
            Duration::from_nanos(ceiling / 2)
        );
    }

    #[test]
    fn reset_starts_over_from_the_base() {
        let mut backoff = Backoff::new(SECOND, Duration::from_secs(300));
        for _ in 0..30 {
            backoff.next_delay(0);
        }
        assert_eq!(backoff.ceiling(), Duration::from_secs(300));
        backoff.reset();
        assert_eq!(backoff.ceiling(), SECOND);
    }

    #[test]
    fn at_cap_waits_between_half_the_cap_and_the_cap() {
        let backoff = Backoff::new(SECOND, Duration::from_secs(600));
        for random in [0, 1, 299_999_999_999, u64::MAX] {
            let delay = backoff.at_cap(random);
            assert!(delay >= Duration::from_secs(300), "{delay:?}");
            assert!(delay <= Duration::from_secs(600), "{delay:?}");
        }
    }

    #[test]
    fn a_zero_base_stays_at_zero() {
        let mut backoff = Backoff::new(Duration::ZERO, Duration::from_secs(5));
        for _ in 0..100 {
            assert_eq!(backoff.next_delay(u64::MAX), Duration::ZERO);
        }
    }

    #[test]
    fn a_huge_attempt_count_never_overflows() {
        let mut backoff = Backoff::new(Duration::from_secs(u64::MAX / 4), Duration::MAX);
        for _ in 0..200 {
            backoff.next_delay(u64::MAX);
        }
        assert_eq!(backoff.ceiling(), Duration::MAX);
    }
}
