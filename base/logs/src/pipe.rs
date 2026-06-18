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

pub(crate) async fn pipe(
    reader: &mut Reader,
    server: &logs_args::server::Client,
    minimum_severity: u16,
    follow: bool,
) -> capnp::Result<()> {
    let stop = server.stop_request().send().promise;

    let streaming = async {
        let mut batch: Vec<Builder<HeapAllocator>> = Vec::new();
        let mut outcome: capnp::Result<()> = Ok(());
        loop {
            embassy_futures::yield_now().await;

            if outcome.is_err() || (!follow && reader.caught_up_to_subscription()) {
                break outcome;
            }

            let mut examined = 0;
            while batch.len() < MAX_BATCH && examined < MAX_EXAMINED {
                let Some(signal) = reader.try_read() else {
                    if !batch.is_empty() {
                        break;
                    } else {
                        embassy_futures::yield_now().await;
                        continue;
                    }
                };
                examined += 1;
                if keeps(&signal, minimum_severity)? {
                    batch.push(signal);
                }
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
                Err(error) if error.kind == capnp::ErrorKind::Disconnected => {
                    outcome = Err(error);
                }
                Err(error) => {
                    tracing::warn!(%error, signal_count = batch.len(), "batch failed to send; retrying");
                }
            }
        }
    };

    match select(stop, streaming).await {
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
