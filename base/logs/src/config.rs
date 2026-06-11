//! Lane layout for the logs buffer.

use alloc::vec::Vec;
use tracing::Level;

/// Default per-lane byte capacity.
const DEFAULT_BYTES: usize = 262144;
/// Default per-lane record capacity (slot count).
const DEFAULT_RECORDS: usize = 4096;

/// One lane of the log buffer: an independent ring with its own budget.
///
/// The levels routed to a lane share it — they evict each other to stay within
/// the lane's budget — but a lane never evicts another lane's records. Eviction
/// is bounded by **both** the byte capacity and the record (slot) capacity,
/// whichever binds first.
#[derive(Clone, Debug)]
pub struct LaneConfig {
    /// Levels whose records are stored in this lane. A level routed by no lane is
    /// dropped; if more than one lane lists it, the earliest lane in
    /// [`LogsConfig::lanes`] wins.
    pub levels: Vec<Level>,
    /// Maximum retained bytes; the lane evicts its own oldest to stay within it.
    pub byte_capacity: usize,
    /// Maximum retained records — the lane's fixed slot count.
    pub record_capacity: usize,
}

/// The log buffer's lane layout.
#[derive(Clone, Debug)]
pub struct LogsConfig {
    /// The lanes, in priority order (earlier lanes win a level listed by several).
    pub lanes: Vec<LaneConfig>,
}

impl Default for LogsConfig {
    /// One lane per level, each `262144` bytes / `4096` records.
    fn default() -> Self {
        let lane = |level| LaneConfig {
            levels: Vec::from([level]),
            byte_capacity: DEFAULT_BYTES,
            record_capacity: DEFAULT_RECORDS,
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
