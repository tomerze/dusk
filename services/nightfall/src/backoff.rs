use rand::Rng;
use std::time::Duration;

pub fn ceiling(attempt: u32, base: Duration, cap: Duration) -> Duration {
    let factor = 1u32.checked_shl(attempt.min(31)).unwrap_or(u32::MAX);
    base.saturating_mul(factor).min(cap)
}

pub fn full_jitter(attempt: u32, base: Duration, cap: Duration) -> Duration {
    let ceiling = ceiling(attempt, base, cap);
    let milliseconds = ceiling.as_millis().min(u128::from(u64::MAX)) as u64;
    Duration::from_millis(rand::rng().random_range(0..=milliseconds))
}

pub fn jittered(period: Duration, fraction: f64) -> Duration {
    let spread = rand::rng().random_range(-fraction..=fraction);
    period.mul_f64((1.0 + spread).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_exponentially_up_to_the_cap() {
        let base = Duration::from_secs(1);
        let cap = Duration::from_secs(300);
        assert_eq!(ceiling(0, base, cap), Duration::from_secs(1));
        assert_eq!(ceiling(3, base, cap), Duration::from_secs(8));
        assert_eq!(ceiling(9, base, cap), Duration::from_secs(300));
        assert_eq!(ceiling(200, base, cap), Duration::from_secs(300));
        for attempt in 0..40 {
            assert!(full_jitter(attempt, base, cap) <= ceiling(attempt, base, cap));
        }
    }

    #[test]
    fn jitters_a_period_within_its_fraction() {
        for _ in 0..1000 {
            let value = jittered(Duration::from_secs(30), 0.2);
            assert!(value >= Duration::from_secs(24) && value <= Duration::from_secs(36));
        }
    }
}
