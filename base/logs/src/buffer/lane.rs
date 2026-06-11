use crate::config::LaneConfig;
use alloc::boxed::Box;
use alloc::vec::Vec;
use portable_atomic::{AtomicU8, AtomicU32, AtomicU64};

use super::LANE_SEQUENCE_WRITING;

/// `head` and `tail` are monotonic positions, never wrapped; physical slots are
/// `position % capacity`. u64 wrap-around is assumed unreachable.
pub(super) struct Ring<T> {
    pub(super) capacity: u64,
    pub(super) data: Box<[T]>,
    pub(super) head: AtomicU64,
    pub(super) tail: AtomicU64,
}

impl<T> Ring<T> {
    /// A `capacity`-slot ring with each slot built by `init` and the head/tail at 0.
    fn new(capacity: u64, init: impl Fn() -> T) -> Self {
        Ring {
            capacity,
            data: (0..capacity)
                .map(|_| init())
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
        }
    }
}

pub(super) struct Descriptor {
    pub(super) lane_sequence: AtomicU64,
    pub(super) global_sequence: AtomicU64,
    pub(super) level: AtomicU8,
    pub(super) data_position: AtomicU64,
    pub(super) data_length: AtomicU32,
}

pub(super) struct Lane {
    pub(super) descriptors: Ring<Descriptor>,
    pub(super) logs: Ring<AtomicU8>,
}

impl Lane {
    /// Build a lane's two rings from a [`LaneConfig`]. Errors on a zero capacity
    /// (the modulo indexing would divide by zero) and on a `byte_capacity` above
    /// `u32::MAX` (a record's length is stored in a `u32` descriptor field).
    pub(super) fn from_config(config: &LaneConfig) -> anyhow::Result<Self> {
        for (name, capacity) in [
            ("record_capacity", config.record_capacity),
            ("byte_capacity", config.byte_capacity),
        ] {
            if capacity == 0 {
                anyhow::bail!("{name} must be at least 1, got 0");
            }
        }
        if config.byte_capacity as u64 > u32::MAX as u64 {
            anyhow::bail!(
                "byte_capacity must fit in a u32 (the descriptor's record-length field), got {}",
                config.byte_capacity
            );
        }
        Ok(Lane {
            descriptors: Ring::new(config.record_capacity as u64, || Descriptor {
                lane_sequence: AtomicU64::new(LANE_SEQUENCE_WRITING),
                global_sequence: AtomicU64::new(0),
                level: AtomicU8::new(0),
                data_position: AtomicU64::new(0),
                data_length: AtomicU32::new(0),
            }),
            logs: Ring::new(config.byte_capacity as u64, || AtomicU8::new(0)),
        })
    }
}
