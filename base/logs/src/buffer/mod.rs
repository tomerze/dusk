mod enrich;
mod io;
mod lane;
mod notify;

pub use io::{Reader, Writer};

use crate::config::LogsConfig;
use alloc::boxed::Box;
use alloc::vec::Vec;
use anyhow::Context;
use dusk_capnp::dusk_capnp::dusk;
use dusk_program::embassy_time::Instant;
use lane::Lane;
use notify::Notify;
use portable_atomic::AtomicU64;
use portable_atomic_util::Arc;
use tracing::Level;

/// The mid-write sentinel. It collides with a real descriptor position only at
/// `u64::MAX` — unreachable (see the no-wrap note on [`lane::Ring`]).
const LANE_SEQUENCE_WRITING: u64 = u64::MAX;

pub(crate) const LEVELS: [Level; 5] = [
    Level::ERROR,
    Level::WARN,
    Level::INFO,
    Level::DEBUG,
    Level::TRACE,
];

pub(crate) fn level_index(level: Level) -> usize {
    LEVELS
        .iter()
        .position(|&candidate| candidate == level)
        .unwrap_or(LEVELS.len() - 1)
}

#[derive(Clone, Copy, Debug)]
pub enum StartPosition {
    /// Yield every record currently retained (oldest-first) before following new
    /// ones — a `tail -f`-style replay-then-tail.
    Replay,
    /// Skip the retained history; only yield records pushed after subscribing.
    Live,
}

pub(in crate::buffer) struct LogBufferInner {
    lanes: Box<[Lane]>,
    level_to_lane: [Option<usize>; 5],
    global_sequence: AtomicU64,
    notify: Notify,
    dropped_no_lane: AtomicU64,
    dropped_oversize: AtomicU64,
    write_failures: AtomicU64,
}

/// Records discarded without being stored, by cause. The write path never fails
/// for these — the counters are the only trace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DropCounts {
    /// Level routed to no lane.
    pub no_lane: u64,
    /// Bigger than the lane's whole byte arena.
    pub oversize: u64,
    /// Serialization failed inside `Writer::write` (the error is also returned).
    pub write_failures: u64,
}

/// A handle is a pointer-clone: clones share the same lanes. Mint per-producer
/// [`Writer`]s and per-consumer [`Reader`]s from any clone.
#[derive(Clone)]
pub struct LogBuffer {
    inner: Arc<LogBufferInner>,
}

impl LogBuffer {
    pub fn new(config: LogsConfig) -> anyhow::Result<Self> {
        let lanes = config
            .lanes
            .iter()
            .enumerate()
            .map(|(lane_index, lane)| {
                Lane::from_config(lane).with_context(|| alloc::format!("lane {lane_index}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?
            .into_boxed_slice();

        // Route each level to the first lane that lists it; uncovered levels drop.
        let mut level_to_lane = [None; 5];
        for (lane_index, lane) in config.lanes.iter().enumerate() {
            for &level in &lane.levels {
                let id = level_index(level);
                if level_to_lane[id].is_none() {
                    level_to_lane[id] = Some(lane_index);
                }
            }
        }

        Ok(Self {
            inner: Arc::new(LogBufferInner {
                lanes,
                level_to_lane,
                global_sequence: AtomicU64::new(0),
                notify: Notify::new(),
                dropped_no_lane: AtomicU64::new(0),
                dropped_oversize: AtomicU64::new(0),
                write_failures: AtomicU64::new(0),
            }),
        })
    }

    /// A producer handle — `Send` but not `Sync`: one per producer thread.
    pub fn writer(&self) -> Writer {
        Writer::new(self.inner.clone())
    }

    /// Whether `level` routes to a lane; records at unrouted levels are dropped.
    pub(crate) fn routes(&self, level: Level) -> bool {
        self.inner.level_to_lane[level_index(level)].is_some()
    }

    /// A consumer with the wall-clock offset taken from the node via one
    /// `Dusk.time` RPC. The offset is frozen for the reader's lifetime: a later
    /// `Dusk.settime` skews every timestamp this reader subsequently enriches.
    pub async fn reader(
        &self,
        start: StartPosition,
        dusk_client: dusk::Client,
    ) -> capnp::Result<Reader> {
        let reply = dusk_client.time_request().send().promise.await?;
        // Saturate: an unset wall-clock yields a 0 offset (embassy-relative
        // timestamps) instead of underflowing.
        let offset_from_unix_time_ms = reply
            .get()?
            .get_unix_time_ms()
            .saturating_sub(Instant::now().as_millis());
        Ok(self.reader_with_offset(start, offset_from_unix_time_ms))
    }

    /// A consumer with a caller-supplied wall-clock offset (Unix ms minus the
    /// node's monotonic ms at the same instant) — for contexts with no RPC
    /// session. An offset of 0 yields embassy-relative timestamps.
    pub fn reader_with_offset(
        &self,
        start: StartPosition,
        offset_from_unix_time_ms: u64,
    ) -> Reader {
        Reader::new(self.inner.clone(), start, offset_from_unix_time_ms)
    }

    /// Snapshot of the records discarded so far without being stored.
    pub fn drop_counts(&self) -> DropCounts {
        use core::sync::atomic::Ordering;
        DropCounts {
            no_lane: self.inner.dropped_no_lane.load(Ordering::Relaxed),
            oversize: self.inner.dropped_oversize.load(Ordering::Relaxed),
            write_failures: self.inner.write_failures.load(Ordering::Relaxed),
        }
    }
}
