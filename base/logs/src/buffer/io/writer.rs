//! The [`Writer`]: an append-only producer handle. Lock-free under many writers
//! sharing one buffer.

use crate::buffer::{LANE_SEQUENCE_WRITING, SignalBufferInner, level_index};
use alloc::vec::Vec;
use capnp::message::{self, Builder};
use capnp::serialize_packed;
use core::cell::RefCell;
use core::sync::atomic::Ordering;
use portable_atomic_util::Arc;
use tracing::Level;

/// `Send` but not `Sync` (it owns a packing scratch): mint one `Writer` per
/// producer thread via [`SignalBuffer::writer`](crate::buffer::SignalBuffer::writer).
pub struct Writer {
    inner: Arc<SignalBufferInner>,
    /// Reusable packing scratch - `write` allocates only as the largest signal
    /// seen so far grows it.
    packed: RefCell<Vec<u8>>,
}

impl Writer {
    pub(in crate::buffer) fn new(inner: Arc<SignalBufferInner>) -> Self {
        Writer {
            inner,
            packed: RefCell::new(Vec::new()),
        }
    }

    /// Append a signal to its level's lane. Lock-free under many producers.
    ///
    /// The signal's `time_unix_nano` is the embassy-relative milliseconds the
    /// caller stamped at assembly (enrichment turns it into real Unix time on the
    /// way out).
    ///
    /// Unrouted-level and oversize signals are dropped with `Ok(())`, counted in
    /// [`SignalBuffer::drop_counts`](crate::buffer::SignalBuffer::drop_counts).
    pub fn write<Allocator: message::Allocator>(
        &self,
        level: Level,
        message: &mut Builder<Allocator>,
    ) -> capnp::Result<()> {
        let level_id = level_index(level);
        let lane_index = match self.inner.level_to_lane[level_id] {
            Some(lane_index) => lane_index,
            None => {
                self.inner.dropped_no_lane.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
        };

        // Fall back to a one-off buffer if `write` is re-entered on this same
        // writer, so the scratch RefCell can't double-borrow and panic.
        let mut fallback = Vec::new();
        let mut borrowed = self.packed.try_borrow_mut().ok();
        let packed: &mut Vec<u8> = match borrowed.as_deref_mut() {
            Some(reused) => {
                reused.clear();
                reused
            }
            None => &mut fallback,
        };
        serialize_packed::write_message(&mut *packed, message).inspect_err(|_| {
            self.inner.write_failures.fetch_add(1, Ordering::Relaxed);
        })?;
        let size = packed.len() as u64;

        let lane = &self.inner.lanes[lane_index];
        if size > lane.logs.capacity {
            self.inner.dropped_oversize.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }

        // Claim a global sequence, a disjoint byte run, and a descriptor slot.
        let sequence = self.inner.global_sequence.fetch_add(1, Ordering::AcqRel);
        let data_position = lane.logs.head.fetch_add(size, Ordering::AcqRel);
        let descriptor_position = lane.descriptors.head.fetch_add(1, Ordering::AcqRel);

        if descriptor_position >= lane.descriptors.capacity {
            lane.overwritten.fetch_add(1, Ordering::Relaxed);
        }

        // Reclaim-before-reuse (drop-oldest): advance both tails past what we're
        // about to overwrite *first*, so a reader still on the old occupants
        // skips them or rejects its copy. fetch_max keeps concurrent advances
        // monotonic.
        lane.descriptors.tail.fetch_max(
            (descriptor_position + 1).saturating_sub(lane.descriptors.capacity),
            Ordering::AcqRel,
        );
        lane.logs.tail.fetch_max(
            (data_position + size).saturating_sub(lane.logs.capacity),
            Ordering::AcqRel,
        );

        // Seqlock write: close the slot, fill it, copy the payload, publish. The
        // first fence orders the field stores after WRITING (a Release store does
        // not order what follows it); the second orders the tail advances and
        // fields ahead of the byte overwrite. Each pairs with a reader-side
        // Acquire fence, making the reader's re-checks catch any torn read.
        let descriptor =
            &lane.descriptors.data[(descriptor_position % lane.descriptors.capacity) as usize];
        descriptor
            .lane_sequence
            .store(LANE_SEQUENCE_WRITING, Ordering::Release);
        core::sync::atomic::fence(Ordering::Release);
        descriptor
            .global_sequence
            .store(sequence, Ordering::Relaxed);
        descriptor.level.store(level_id as u8, Ordering::Relaxed);
        descriptor
            .data_position
            .store(data_position, Ordering::Relaxed);
        descriptor.data_length.store(size as u32, Ordering::Relaxed);
        core::sync::atomic::fence(Ordering::Release);
        for offset in 0..size {
            let index = ((data_position + offset) % lane.logs.capacity) as usize;
            lane.logs.data[index].store(packed[offset as usize], Ordering::Relaxed);
        }
        descriptor
            .lane_sequence
            .store(descriptor_position, Ordering::Release);

        self.inner.notify.notify();
        Ok(())
    }
}
