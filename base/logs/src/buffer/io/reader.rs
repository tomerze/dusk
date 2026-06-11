//! The [`Reader`]: a non-destructive consumer that merges the lanes in
//! global-sequence order and copies each record out of the arena under the
//! descriptor seqlock.

use crate::buffer::enrich::unpack_and_enrich;
use crate::buffer::{LEVELS, LogBufferInner, StartPosition, level_index};
use alloc::boxed::Box;
use alloc::vec::Vec;
use capnp::message::{Builder, HeapAllocator};
use core::sync::atomic::Ordering;
use portable_atomic_util::Arc;
use tracing::Level;

pub enum LogEntry {
    /// A log record, enriched with its severity and real timestamps.
    Record(Builder<HeapAllocator>),
    /// Records this reader will never yield — evicted, reclaimed mid-read, or
    /// unparseable. `levels` are the levels of the lane that lost them.
    Gap { missed: u64, levels: Vec<Level> },
}

enum Read {
    Record { level: Level },
    Reclaimed,
    Retry,
}

pub(in crate::buffer) enum Step {
    Record {
        level: Level,
    },
    Gap {
        missed: u64,
        lane_index: usize,
    },
    /// Nothing yieldable right now — caught up, or the next candidate is
    /// mid-write. Either way the next `write`'s notify resolves it.
    Pending,
}

pub struct Reader {
    inner: Arc<LogBufferInner>,
    /// Per-lane next descriptor position to read.
    cursor: Box<[u64]>,
    offset_from_unix_time_ms: u64,
    /// Scratch for the bytes copied out by the last `Step::Record`.
    packed: Vec<u8>,
}

impl Reader {
    pub(in crate::buffer) fn new(
        inner: Arc<LogBufferInner>,
        start: StartPosition,
        offset_from_unix_time_ms: u64,
    ) -> Self {
        let cursor = inner
            .lanes
            .iter()
            .map(|lane| match start {
                StartPosition::Replay => lane.descriptors.tail.load(Ordering::Acquire),
                StartPosition::Live => lane.descriptors.head.load(Ordering::Acquire),
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Reader {
            inner,
            cursor,
            offset_from_unix_time_ms,
            packed: Vec::new(),
        }
    }

    /// The next entry, parking until one arrives. An unparseable record is
    /// consumed and reported as a [`LogEntry::Gap`].
    pub async fn read(&mut self) -> LogEntry {
        let inner = self.inner.clone();
        loop {
            // Sample before scanning: a write landing mid-scan moves the version,
            // so the park returns immediately and we re-scan.
            let version = inner.notify.version();
            match self.try_step() {
                Step::Pending => inner.notify.changed(version).await,
                step => return self.entry(step),
            }
        }
    }

    /// The non-parking partner of [`read`](Self::read): the next entry if one is
    /// immediately available.
    pub fn try_read(&mut self) -> Option<LogEntry> {
        match self.try_step() {
            Step::Pending => None,
            step => Some(self.entry(step)),
        }
    }

    fn entry(&mut self, step: Step) -> LogEntry {
        match step {
            Step::Record { level } => {
                match unpack_and_enrich(&self.packed, level, self.offset_from_unix_time_ms) {
                    Ok(record) => LogEntry::Record(record),
                    // The cursor already moved past it; erring would make one
                    // corrupt record look fatal to the whole subscription.
                    Err(error) => {
                        tracing::warn!(%error, "skipping an unparseable log record");
                        let lane_index = self.inner.level_to_lane[level_index(level)]
                            .expect("a yielded record's level is always routed");
                        LogEntry::Gap {
                            missed: 1,
                            levels: self.inner.lane_levels(lane_index),
                        }
                    }
                }
            }
            Step::Gap { missed, lane_index } => LogEntry::Gap {
                missed,
                levels: self.inner.lane_levels(lane_index),
            },
            Step::Pending => unreachable!("entry() is only called with Record / Gap steps"),
        }
    }

    /// One synchronous read attempt: merge the lanes in global-sequence order and
    /// return the next record's level (its bytes land in the scratch), a gap, or
    /// `Pending`.
    pub(in crate::buffer) fn try_step(&mut self) -> Step {
        let mut best: Option<(usize, u64)> = None; // (lane, sequence)

        for lane_index in 0..self.inner.lanes.len() {
            let lane = &self.inner.lanes[lane_index];
            let head = lane.descriptors.head.load(Ordering::Acquire);
            let oldest = lane.descriptors.tail.load(Ordering::Acquire);
            let cursor = self.cursor[lane_index];

            if cursor < oldest {
                // Fell behind; this lane's oldest descriptors were evicted.
                let missed = oldest - cursor;
                self.cursor[lane_index] = oldest;
                return Step::Gap { missed, lane_index };
            }
            if cursor >= head {
                continue;
            }
            let descriptor = &lane.descriptors.data[(cursor % lane.descriptors.capacity) as usize];
            // Accept the slot only when it publishes *our* position; anything else
            // is mid-write or a not-yet-written claim.
            if descriptor.lane_sequence.load(Ordering::Acquire) != cursor {
                continue;
            }
            let sequence = descriptor.global_sequence.load(Ordering::Relaxed);
            if best.is_none_or(|(_, b)| sequence < b) {
                best = Some((lane_index, sequence));
            }
        }

        let Some((lane_index, _sequence)) = best else {
            return Step::Pending;
        };

        match self.read_descriptor(lane_index) {
            Read::Record { level } => {
                self.cursor[lane_index] += 1;
                Step::Record { level }
            }
            Read::Reclaimed => {
                // The descriptor is still ours, but the arena lapped its bytes.
                self.cursor[lane_index] += 1;
                Step::Gap {
                    missed: 1,
                    lane_index,
                }
            }
            Read::Retry => Step::Pending, // overwritten / reclaimed mid-copy
        }
    }

    /// Copy the record at `lane_index`'s cursor out of the arena, validating the
    /// descriptor seqlock and that the bytes weren't reclaimed mid-copy.
    fn read_descriptor(&mut self, lane_index: usize) -> Read {
        let cursor = self.cursor[lane_index];
        let lane = &self.inner.lanes[lane_index];
        let descriptor = &lane.descriptors.data[(cursor % lane.descriptors.capacity) as usize];

        if descriptor.lane_sequence.load(Ordering::Acquire) != cursor {
            return Read::Retry;
        }
        let data_position = descriptor.data_position.load(Ordering::Relaxed);
        let data_length = descriptor.data_length.load(Ordering::Relaxed) as u64;
        let level = LEVELS[descriptor.level.load(Ordering::Relaxed) as usize];

        // Pairs with the producer's post-WRITING Release fence: if any field read
        // was torn by a rewrite, this re-check sees the slot move and rejects.
        core::sync::atomic::fence(Ordering::Acquire);
        if descriptor.lane_sequence.load(Ordering::Relaxed) != cursor {
            return Read::Retry;
        }

        if data_position < lane.logs.tail.load(Ordering::Acquire) {
            return Read::Reclaimed;
        }

        self.packed.clear();
        self.packed.reserve(data_length as usize);
        for offset in 0..data_length {
            let index = ((data_position + offset) % lane.logs.capacity) as usize;
            self.packed
                .push(lane.logs.data[index].load(Ordering::Relaxed));
        }

        // Pairs with the producer's pre-copy Release fence: a reader that copied
        // an overwritten or reclaimed byte fails one of these re-checks.
        core::sync::atomic::fence(Ordering::Acquire);
        if descriptor.lane_sequence.load(Ordering::Relaxed) == cursor
            && data_position >= lane.logs.tail.load(Ordering::Relaxed)
        {
            Read::Record { level }
        } else {
            Read::Retry
        }
    }
}
