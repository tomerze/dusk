//! The [`Reader`]: a non-destructive consumer that merges the lanes in
//! global-sequence order and copies each record out of the arena under the
//! descriptor seqlock.

use crate::buffer::enrich::unpack_and_enrich;
use crate::buffer::{LEVELS, LogBufferInner, StartPosition};
use alloc::boxed::Box;
use alloc::vec::Vec;
use capnp::message::{Builder, HeapAllocator};
use core::sync::atomic::Ordering;
use dusk_program::embassy_futures;
use portable_atomic_util::Arc;
use tracing::Level;

enum Read {
    Record { level: Level },
    Reclaimed,
    Retry,
}

pub struct Reader {
    inner: Arc<LogBufferInner>,
    /// Per-lane next descriptor position to read.
    cursor: Box<[u64]>,
    offset_from_unix_time_ms: u64,
    /// Scratch for the bytes copied out by the last accepted descriptor.
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

    /// The next record, parking until one arrives, enriched with its severity
    /// and real timestamps. Records this reader will never yield — evicted,
    /// reclaimed mid-read, or unparseable — are skipped.
    pub async fn read(&mut self) -> Builder<HeapAllocator> {
        loop {
            // Sample before scanning: a write landing mid-scan moves the version,
            // so the park returns immediately and we re-scan.
            let version = self.inner.notify.version();
            if let Some(record) = self.try_read() {
                return record;
            }
            // Under sustained writer pressure the version has always already
            // moved, making the park below ready on its first poll — which
            // never yields. Without this unconditional yield the loop starves
            // the single-threaded executor.
            embassy_futures::yield_now().await;
            self.inner.notify.changed(version).await;
        }
    }

    /// The non-parking partner of [`read`](Self::read): the next record if one
    /// is immediately available.
    pub fn try_read(&mut self) -> Option<Builder<HeapAllocator>> {
        loop {
            let level = self.try_step()?;
            match unpack_and_enrich(&self.packed, level, self.offset_from_unix_time_ms) {
                Ok(record) => return Some(record),
                // The cursor already moved past it; erring would make one
                // corrupt record look fatal to the whole subscription.
                Err(error) => tracing::warn!(%error, "skipping an unparseable log record"),
            }
        }
    }

    /// One synchronous read attempt: merge the lanes in global-sequence order
    /// and return the next record's level (its bytes land in the scratch).
    /// Evicted and reclaimed records are stepped over; `None` means nothing is
    /// yieldable right now — caught up, or the next candidate is mid-write —
    /// and the next `write`'s notify resolves it.
    fn try_step(&mut self) -> Option<Level> {
        loop {
            let mut best: Option<(usize, u64)> = None; // (lane, sequence)

            for lane_index in 0..self.inner.lanes.len() {
                let lane = &self.inner.lanes[lane_index];
                let head = lane.descriptors.head.load(Ordering::Acquire);
                let oldest = lane.descriptors.tail.load(Ordering::Acquire);
                if self.cursor[lane_index] < oldest {
                    // Fell behind; this lane's oldest descriptors were evicted.
                    self.cursor[lane_index] = oldest;
                }
                let cursor = self.cursor[lane_index];
                if cursor >= head {
                    continue;
                }
                let descriptor =
                    &lane.descriptors.data[(cursor % lane.descriptors.capacity) as usize];
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

            let (lane_index, _sequence) = best?;

            match self.read_descriptor(lane_index) {
                Read::Record { level } => {
                    self.cursor[lane_index] += 1;
                    return Some(level);
                }
                // The descriptor is still ours, but the arena lapped its bytes.
                Read::Reclaimed => self.cursor[lane_index] += 1,
                Read::Retry => return None, // overwritten / reclaimed mid-copy
            }
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
