//! The node side of the log stream: piping the buffer into the client's
//! stream server, one `send` per batch, until the client answers the `stop`
//! long-poll. It moves signals from one buffer (the node's ring) to another
//! (the client's stream).

use crate::buffer::Reader;
use crate::logs_capnp::logs_args;
use crate::logs_capnp::signal;
use alloc::vec::Vec;
use capnp::message::{Builder, HeapAllocator};
use dusk_program::embassy_futures;
use dusk_program::embassy_futures::select::{Either, select};

/// Cap on signals per `send`: bounds the message while letting a replaying
/// client catch up in few round-trips.
const MAX_BATCH: usize = 64;

/// Cap on signals *examined* per iteration: with a severity filter, a flood
/// of filtered-out signals fills no batch — this bounds the work between
/// yields regardless.
const MAX_EXAMINED: usize = 256;

/// Pipe `reader` into the client's `send`, one batch per call: drain
/// whatever the buffer holds (up to [`MAX_BATCH`]) and send it. Awaiting the
/// send is the only gate — capnp's streaming flow control resolves it under
/// credit and withholds otherwise, and while it withholds, the ring gathers
/// the next batch, so batch sizes follow the client's absorption rate on
/// their own. A failed send keeps its batch and retries, paced by the
/// round-trip. With `follow`, runs until the client answers the `stop`
/// long-poll (the normal end) and only the connection dying ends it with an
/// error; without `follow`, it also ends — cleanly — once the buffer is
/// drained, a bounded replay-only snapshot.
pub(crate) async fn pipe(
    reader: &mut Reader,
    server: &logs_args::server::Client,
    minimum_severity: u16,
    follow: bool,
) -> capnp::Result<()> {
    let stop = server.stop_request().send().promise;

    let streaming = async {
        let mut batch: Vec<Builder<HeapAllocator>> = Vec::new();
        loop {
            // Drain a bounded slice of whatever is buffered. MAX_EXAMINED
            // counts signals looked at, not kept: with a severity filter a
            // flood of discarded signals fills no batch but must still reach
            // a yield.
            let mut examined = 0;
            while batch.len() < MAX_BATCH && examined < MAX_EXAMINED {
                let Some(signal) = reader.try_read() else {
                    break;
                };
                examined += 1;
                if keeps(&signal, minimum_severity)? {
                    batch.push(signal);
                }
            }

            if batch.is_empty() {
                if examined == 0 {
                    if !follow && reader.caught_up_to_subscription() {
                        // Replay-only: the retained history is drained — the
                        // bounded snapshot is complete.
                        return Ok(());
                    }
                    // Nothing readable yet: park for the next signal.
                    let signal = reader.read().await;
                    if keeps(&signal, minimum_severity)? {
                        batch.push(signal);
                    }
                } else {
                    // Worked a full slice and kept none: yield before
                    // scanning on, so a filtered flood cannot hog the
                    // executor.
                    embassy_futures::yield_now().await;
                }
                continue;
            }

            let mut request = server.send_request();
            let mut list = request.get().init_entries(batch.len() as u32);
            for (index, signal) in batch.iter().enumerate() {
                list.set_with_caveats(
                    index as u32,
                    signal.get_root_as_reader::<signal::Reader>()?,
                )?;
            }
            match request.send().await {
                Ok(()) => batch.clear(),
                // The connection is gone; no retry can ever land.
                Err(error) if error.kind == capnp::ErrorKind::Disconnected => {
                    return Err(error);
                }
                // Keep the batch and send it again; the failed round-trip
                // itself paces the retry.
                Err(error) => {
                    tracing::debug!(%error, "a log batch failed to send; retrying");
                }
            }
            // One guaranteed suspension per send: under flow-control credit
            // `send` resolves immediately and the loop would never yield on
            // its own.
            embassy_futures::yield_now().await;

            // Replay-only: stop once the reader has passed the subscription
            // head, even while the node keeps logging — everything beyond it is
            // live, not the history this snapshot is for.
            if !follow && reader.caught_up_to_subscription() {
                return Ok(());
            }
        }
    };

    match select(stop, streaming).await {
        // The long-poll answered is the destination closing — the normal
        // end. The long-poll failing is the connection dying: nothing can be
        // delivered either way.
        Either::First(stop_result) => stop_result.map(|_| ()),
        Either::Second(streaming_result) => streaming_result,
    }
}

/// Whether the signal passes the severity floor; an unreadable severity fails
/// open. The level is the signal's envelope severity (the lane it rode), so logs
/// and spans filter alike.
fn keeps(signal: &Builder<HeapAllocator>, minimum_severity: u16) -> capnp::Result<bool> {
    if minimum_severity == 0 {
        return Ok(true);
    }
    let severity = signal
        .get_root_as_reader::<signal::Reader>()?
        .get_severity_number()
        .map(|severity| severity as u16)
        .unwrap_or(u16::MAX);
    Ok(severity >= minimum_severity)
}
