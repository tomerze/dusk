//! Lane layout for the logs buffer.

use alloc::vec::Vec;
use tracing::Level;

/// Default per-lane byte capacity.
const DEFAULT_BYTES: usize = 262144;
/// Default per-lane signal capacity (slot count).
const DEFAULT_SIGNALS: usize = 4096;

#[derive(Clone, Debug)]
pub struct LaneConfig {
    /// Levels whose signals are stored in this lane. A level routed by no lane is
    /// dropped; if more than one lane lists it, the earliest lane in
    /// [`LogsConfig::lanes`] wins.
    pub levels: Vec<Level>,
    /// Maximum retained bytes; the lane evicts its own oldest to stay within it.
    pub byte_capacity: usize,
    /// Maximum retained signals - the lane's fixed slot count.
    pub signal_capacity: usize,
}

#[derive(Clone, Debug)]
pub struct LogsConfig {
    pub lanes: Vec<LaneConfig>,
}

impl Default for LogsConfig {
    /// One lane per level, each `262144` bytes / `4096` signals.
    fn default() -> Self {
        let lane = |level| LaneConfig {
            levels: Vec::from([level]),
            byte_capacity: DEFAULT_BYTES,
            signal_capacity: DEFAULT_SIGNALS,
        };
        Self {
            lanes: Vec::from([
                lane(Level::ERROR),
                lane(Level::WARN),
                lane(Level::INFO),
                lane(Level::DEBUG),
                lane(Level::TRACE),
            ]),
        }
    }
}
