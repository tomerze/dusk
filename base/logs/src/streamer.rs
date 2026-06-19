use crate::buffer::Reader;
use crate::logs_capnp::logs_args;
use crate::logs_capnp::signal;
use crate::logs_capnp::signal_batch;
use alloc::rc::Rc;
use alloc::vec::Vec;
use capnp::capability::Promise;
use capnp::message::{Builder, HeapAllocator};
use core::sync::atomic::Ordering;
use dusk_capnp::capnp_rpc;
use dusk_program::embassy_futures;
use dusk_program::embassy_futures::select::{Either, select};
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::embassy_sync::channel::Channel;
use portable_atomic::AtomicUsize;

type SignalBuilder = Builder<HeapAllocator>;

/// A queue of batches to be resent when their ack is not received. Its capacity
/// is the in-flight bound, so it can never hold more than is in flight.
type ResendQueue<const MAX_INFLIGHT: usize> =
    Channel<CriticalSectionRawMutex, Vec<SignalBuilder>, MAX_INFLIGHT>;

struct AckServer<const MAX_INFLIGHT: usize> {
    acknowledged: bool,
    signals: Vec<SignalBuilder>,
    resend_queue: Rc<ResendQueue<MAX_INFLIGHT>>,
    inflight: Rc<AtomicUsize>,
}

impl<const MAX_INFLIGHT: usize> signal_batch::ack::Server for AckServer<MAX_INFLIGHT> {
    fn ack(
        &mut self,
        _params: signal_batch::ack::AckParams,
        _results: signal_batch::ack::AckResults,
    ) -> Promise<(), capnp::Error> {
        self.acknowledged = true;
        Promise::ok(())
    }
}

impl<const MAX_INFLIGHT: usize> Drop for AckServer<MAX_INFLIGHT> {
    fn drop(&mut self) {
        if self.acknowledged {
            self.inflight.fetch_sub(1, Ordering::Relaxed);
            return;
        }

        let signals = core::mem::take(&mut self.signals);
        if self.resend_queue.try_send(signals).is_err() {
            tracing::error!("resend queue full; dropping an unacknowledged batch");
        }
    }
}

pub struct Streamer<const MAX_INFLIGHT: usize> {
    /// The maximum number of signals per batch.
    max_signals_per_batch: usize,
    /// The maximum number of signals examined per drain, to bound work when a
    /// severity filter rejects most of what it sees.
    max_examined_signals_per_drain: usize,
    /// Batches awaiting a re-send because their ack never arrived.
    resend_queue: Rc<ResendQueue<MAX_INFLIGHT>>,
    /// Batches sent but not yet acknowledged; the backpressure window, bounded by
    /// `MAX_INFLIGHT`.
    inflight: Rc<AtomicUsize>,
}

impl<const MAX_INFLIGHT: usize> Streamer<MAX_INFLIGHT> {
    pub(crate) fn new(max_signals_per_batch: usize, max_examined_signals_per_drain: usize) -> Self {
        Streamer {
            max_signals_per_batch,
            max_examined_signals_per_drain,
            resend_queue: Rc::new(Channel::new()),
            inflight: Rc::new(AtomicUsize::new(0)),
        }
    }

    pub(crate) async fn stream(
        &self,
        reader: &mut Reader,
        server: &logs_args::server::Client,
        minimum_severity: u16,
        follow: bool,
    ) -> capnp::Result<()> {
        self.resend_queue.clear();
        self.inflight.store(0, Ordering::Relaxed);

        let stop = server.stop_request().send().promise;

        let streaming = async {
            loop {
                embassy_futures::yield_now().await;
                let outcome = match self.drain(reader, minimum_severity) {
                    Ok(Some(batch)) => self.send_batch(server, batch).await,
                    Ok(None) => Ok(()),
                    Err(error) => Err(error),
                };
                if outcome.is_err() || self.finished(follow, reader) {
                    break outcome;
                }
            }
        };

        match select(stop, streaming).await {
            Either::First(stop_result) => stop_result.map(|_| ()),
            Either::Second(streaming_result) => streaming_result,
        }
    }

    fn drain(
        &self,
        reader: &mut Reader,
        minimum_severity: u16,
    ) -> capnp::Result<Option<Vec<SignalBuilder>>> {
        if let Ok(signals) = self.resend_queue.try_receive() {
            return Ok(Some(signals));
        }
        if self.inflight.load(Ordering::Relaxed) >= MAX_INFLIGHT {
            return Ok(None);
        }

        let mut batch = Vec::new();
        let mut examined = 0;
        while batch.len() < self.max_signals_per_batch
            && examined < self.max_examined_signals_per_drain
        {
            let Some(signal) = reader.try_read() else {
                break;
            };
            examined += 1;
            if keeps(&signal, minimum_severity)? {
                batch.push(signal);
            }
        }
        if batch.is_empty() {
            return Ok(None);
        }

        self.inflight.fetch_add(1, Ordering::Relaxed);
        Ok(Some(batch))
    }

    async fn send_batch(
        &self,
        server: &logs_args::server::Client,
        signals: Vec<SignalBuilder>,
    ) -> capnp::Result<()> {
        let mut request = server.send_request();
        let mut signal_batch = request.get().init_signal_batch();
        {
            let mut list = signal_batch.reborrow().init_signals(signals.len() as u32);
            for (index, signal) in signals.iter().enumerate() {
                list.set_with_caveats(
                    index as u32,
                    signal.get_root_as_reader::<signal::Reader>()?,
                )?;
            }
        }
        let ack = AckServer::<MAX_INFLIGHT> {
            acknowledged: false,
            signals,
            resend_queue: self.resend_queue.clone(),
            inflight: self.inflight.clone(),
        };
        signal_batch.set_ack(capnp_rpc::new_client(ack));

        match request.send().await {
            Ok(()) => Ok(()),
            Err(error) if error.kind == capnp::ErrorKind::Disconnected => Err(error),
            Err(error) => {
                tracing::warn!(%error, "batch failed to send; will retry");
                Ok(())
            }
        }
    }

    fn finished(&self, follow: bool, reader: &Reader) -> bool {
        !follow
            && reader.caught_up_to_subscription()
            && self.resend_queue.is_empty()
            && self.inflight.load(Ordering::Relaxed) == 0
    }
}

/// Whether the signal passes the severity floor; an unreadable severity fails
/// open. The level is the signal's envelope severity (the lane it rode), so logs
/// and spans filter alike.
fn keeps(signal: &SignalBuilder, minimum_severity: u16) -> capnp::Result<bool> {
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
