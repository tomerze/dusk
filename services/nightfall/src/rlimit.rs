use nightfall_membrane::limits::Limits;

pub const RESERVED_DESCRIPTORS: u64 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DescriptorLimit {
    pub soft: u64,
    pub hard: u64,
}

pub fn raise_descriptor_limit() -> std::io::Result<DescriptorLimit> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let previous = limit.rlim_cur;
    if limit.rlim_cur < limit.rlim_max {
        limit.rlim_cur = limit.rlim_max;
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
            let error = std::io::Error::last_os_error();
            tracing::warn!(
                soft = previous,
                hard = limit.rlim_max,
                %error,
                "the open file limit could not be raised to its hard limit"
            );
            limit.rlim_cur = previous;
        }
    }
    let raised = DescriptorLimit {
        soft: limit.rlim_cur,
        hard: limit.rlim_max,
    };
    tracing::info!(
        previous_soft = previous,
        soft = raised.soft,
        hard = raised.hard,
        "open file limit"
    );
    Ok(raised)
}

pub fn reserved_descriptors(limits: &Limits) -> u64 {
    RESERVED_DESCRIPTORS
        .saturating_add(u64::from(limits.max_relayed_connections).saturating_mul(2))
        .saturating_add(u64::from(limits.max_provisioning_connections))
}

pub fn clamp_sessions(configured: u64, reserved: u64, limit: DescriptorLimit) -> u64 {
    let available = limit.soft.saturating_sub(reserved).max(1);
    let clamped = configured.min(available);
    tracing::info!(
        soft_limit = limit.soft,
        reserved,
        max_sessions = clamped,
        "open file budget"
    );
    if clamped < configured {
        tracing::warn!(
            configured,
            clamped,
            soft_limit = limit.soft,
            reserved,
            "fleet.max_sessions is above the open file limit and was lowered"
        );
    }
    clamped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_sessions_to_the_soft_limit_minus_the_reserve() {
        let limit = DescriptorLimit {
            soft: 65_536,
            hard: 65_536,
        };
        assert_eq!(clamp_sessions(100_000, RESERVED_DESCRIPTORS, limit), 55_536);
        assert_eq!(clamp_sessions(1_000, RESERVED_DESCRIPTORS, limit), 1_000);
        let tiny = DescriptorLimit {
            soft: 1024,
            hard: 4096,
        };
        assert_eq!(clamp_sessions(100_000, RESERVED_DESCRIPTORS, tiny), 1);
    }

    #[test]
    fn reserves_descriptors_for_relays_and_provisioning_links() {
        let limits = Limits {
            max_relayed_connections: 100,
            max_provisioning_connections: 50,
            ..Limits::default()
        };
        assert_eq!(reserved_descriptors(&limits), RESERVED_DESCRIPTORS + 250);
        let defaults = reserved_descriptors(&Limits::default());
        assert_eq!(defaults, 40_000);
        let limit = DescriptorLimit {
            soft: 1_048_576,
            hard: 1_048_576,
        };
        assert_eq!(clamp_sessions(2_000_000, defaults, limit), 1_008_576);
    }

    #[test]
    fn raises_the_soft_limit_to_the_hard_limit() {
        let raised = raise_descriptor_limit().unwrap();
        assert_eq!(raised.soft, raised.hard);
    }
}
