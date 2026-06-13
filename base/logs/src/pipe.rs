//! The node side of the log stream: piping the buffer into the client's
//! stream server, one `send` per batch, until the client answers the `stop`
//! long-poll. It moves records from one buffer (the node's ring) to another
//! (the client's stream).

use crate::buffer::Reader;
use crate::log_record_capnp::log_record;
use crate::logs_capnp::logs_args;
use alloc::vec::Vec;
use capnp::message::{Builder, HeapAllocator};
use dusk_program::embassy_futures;
use dusk_program::embassy_futures::select::{Either, select};

/// Cap on records per `send`: bounds the message while letting a replaying
/// client catch up in few round-trips.
const MAX_BATCH: usize = 64;

/// Cap on records *examined* per iteration: with a severity filter, a flood
/// of filtered-out records fills no batch — this bounds the work between
/// yields regardless.
const MAX_EXAMINED: usize = 256;

/// Pipe `reader` into the client's `send`, one batch per call: drain
/// whatever the buffer holds (up to [`MAX_BATCH`]) and send it. Awaiting the
/// send is the only gate — capnp's streaming flow control resolves it under
/// credit and withholds otherwise, and while it withholds, the ring gathers
/// the next batch, so batch sizes follow the client's absorption rate on
/// their own. A failed send keeps its batch and retries, paced by the
/// round-trip. Runs until the client answers the `stop` long-poll (the
/// normal end); only the connection dying ends the stream with an error.
pub(crate) async fn pipe(
    reader: &mut Reader,
    server: &logs_args::server::Client,
    minimum_severity: u16,
) -> capnp::Result<()> {
    let stop = server.stop_request().send().promise;

    let streaming = async {
        let mut batch: Vec<Builder<HeapAllocator>> = Vec::new();
        loop {
            // Drain a bounded slice of whatever is buffered. MAX_EXAMINED
            // counts records looked at, not kept: with a severity filter a
            // flood of discarded records fills no batch but must still reach
            // a yield.
            let mut examined = 0;
            while batch.len() < MAX_BATCH && examined < MAX_EXAMINED {
                let Some(record) = reader.try_read() else { break };
                examined += 1;
                if keeps(&record, minimum_severity)? {
                    batch.push(record);
                }
            }

            if batch.is_empty() {
                if examined == 0 {
                    // Nothing buffered: park for the next record.
                    let record = reader.read().await;
                    if keeps(&record, minimum_severity)? {
                        batch.push(record);
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
            for (index, record) in batch.iter().enumerate() {
                list.set_with_caveats(
                    index as u32,
                    record.get_root_as_reader::<log_record::Reader>()?,
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

/// Whether the record passes the severity floor; an unreadable severity fails
/// open.
fn keeps(record: &Builder<HeapAllocator>, minimum_severity: u16) -> capnp::Result<bool> {
    if minimum_severity == 0 {
        return Ok(true);
    }
    let record = record.get_root_as_reader::<log_record::Reader>()?;
    let severity = record
        .get_severity_number()
        .map(|severity| severity as u16)
        .unwrap_or(u16::MAX);
    Ok(severity >= minimum_severity)
}
