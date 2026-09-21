#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(any(feature = "client", test)), no_std)]

use alloc::rc::Rc;
use dusk_program::embassy_futures::select::{Either, select};
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::{ready::Ready, signal::SignalReceiver};
// `::tracing` (the crate), disambiguated from this crate's own `tracing` module.
use ::tracing::Instrument;

extern crate alloc;
extern crate capnp;
extern crate self as dusk_program_logs;

#[macro_use]
extern crate std;

mod buffer;
mod config;
mod dump;
mod enrich;
mod streamer;
mod tracing;

pub use crate::tracing::BufferLayer;
pub use buffer::{DropCounts, Reader, SignalBuffer, StartPosition, Writer};
pub use config::{LaneConfig, LogsConfig};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const WRITTEN_KEY: u64 = dusk_program_kvs_internal::key_id("logs.written");
const DROPPED_NO_LANE_KEY: u64 = dusk_program_kvs_internal::key_id("logs.dropped_no_lane");
const DROPPED_OVERSIZE_KEY: u64 = dusk_program_kvs_internal::key_id("logs.dropped_oversize");
const WRITE_FAILURES_KEY: u64 = dusk_program_kvs_internal::key_id("logs.write_failures");
const OVERWRITTEN_KEY: u64 = dusk_program_kvs_internal::key_id("logs.overwritten");

#[cfg(feature = "client")]
mod known_keys {
    dusk_program_kvs_internal::known_key!(WRITTEN, "logs.written");
    dusk_program_kvs_internal::known_key!(DROPPED_NO_LANE, "logs.dropped_no_lane");
    dusk_program_kvs_internal::known_key!(DROPPED_OVERSIZE, "logs.dropped_oversize");
    dusk_program_kvs_internal::known_key!(WRITE_FAILURES, "logs.write_failures");
    dusk_program_kvs_internal::known_key!(OVERWRITTEN, "logs.overwritten");
}

dusk_program_proc::metadata!("logs", VERSION, logs_capnp::PROGRAM_ID);

/// OTLP common types (the AnyValue/KeyValue family and InstrumentationScope),
/// compiled from `capnp/otlp/common.capnp`. Shared by `log_record_capnp` and
/// `span_capnp`.
pub mod common_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/otlp/common_capnp.rs"));
}

/// OTel/OTLP log record schema, compiled from `capnp/otlp/log_record.capnp`.
pub mod log_record_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/otlp/log_record_capnp.rs"));
}

/// OTel/OTLP trace schema, compiled from `capnp/otlp/span.capnp`.
pub mod span_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/otlp/span_capnp.rs"));
}

pub use logs_capnp::{FLAG_DUMP, FLAG_FOLLOW, FLAG_REPLAY, logs_args, signal};

#[cfg(feature = "client")]
pub mod client;

#[derive(dusk_program_proc::Launcher)]
pub struct Launcher {
    pub buffer: SignalBuffer,
}

impl Launcher {
    pub fn new(config: LogsConfig) -> anyhow::Result<Self> {
        let buffer = SignalBuffer::new(config)?;
        let _ = ::tracing::subscriber::set_global_default(BufferLayer::new(buffer.clone()));
        Ok(Self { buffer })
    }
}

#[async_trait::async_trait(?Send)]
impl dusk_program::launcher::LauncherMixin for Launcher {
    async fn launch(
        &mut self,
        process_context: ProcessContext,
    ) -> anyhow::Result<Box<dyn dusk_program::process::Process>> {
        Ok(Box::new(
            Process::with_context_and_buffer(process_context, self.buffer.clone()).await?,
        ))
    }
}

#[derive(Clone, dusk_program_proc::Process)]
pub struct Process {
    pub buffer: SignalBuffer,
    minimum_severity: u16,
    start_position: StartPosition,
    follow: bool,
    dump: bool,
    /// Signalled when `main` has finished streaming to the client's stream, so
    /// the portal's `output` can finish the command's output stream and return.
    /// Unused by `dump`, which streams from the portal itself.
    streamer_done: Rc<dusk_program::embassy_sync::signal::Signal<CriticalSectionRawMutex, ()>>,
    #[process_context]
    pub ctx: ProcessContext,
}

impl Process {
    async fn with_context_and_buffer(
        ctx: ProcessContext,
        buffer: SignalBuffer,
    ) -> anyhow::Result<Self> {
        let (minimum_severity, start_position, follow, dump) =
            ctx.program_args
                .with_data::<logs_capnp::logs_args::data::Owned, _, _>(|data| {
                    let minimum_severity = data.get_level().map(|level| level as u16).unwrap_or(0);
                    let flags = data.get_flags();
                    let start_position = if flags & FLAG_REPLAY != 0 {
                        StartPosition::Replay
                    } else {
                        StartPosition::Live
                    };
                    Ok((
                        minimum_severity,
                        start_position,
                        flags & FLAG_FOLLOW != 0,
                        flags & FLAG_DUMP != 0,
                    ))
                })?;
        Ok(Process {
            ctx,
            buffer,
            minimum_severity,
            start_position,
            follow,
            dump,
            streamer_done: Rc::new(dusk_program::embassy_sync::signal::Signal::new()),
        })
    }
}

impl Process {
    /// Record the buffer's counters under the `logs.*` kvs keys.
    async fn publish_counters(&self) {
        let kvs = dusk_program_kvs_internal::get_kvs(self.ctx.namespace.id);
        let counts = self.buffer.drop_counts();
        kvs.set(WRITTEN_KEY, Value::Uint(self.buffer.written()))
            .await;
        kvs.set(DROPPED_NO_LANE_KEY, Value::Uint(counts.no_lane))
            .await;
        kvs.set(DROPPED_OVERSIZE_KEY, Value::Uint(counts.oversize))
            .await;
        kvs.set(WRITE_FAILURES_KEY, Value::Uint(counts.write_failures))
            .await;
        kvs.set(
            OVERWRITTEN_KEY,
            Value::List(counts.overwritten.into_iter().map(Value::Uint).collect()),
        )
        .await;
    }
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    fn portal(&self) -> portal::Client {
        let client: logs_capnp::logs_portal::Client = capnp_rpc::new_client(Portal {
            process: self.clone(),
        });
        client.cast_to::<portal::Client>()
    }

    async fn main(
        &self,
        signal_receiver: SignalReceiver<'async_trait>,
        ready: Ready,
    ) -> anyhow::Result<()> {
        self.publish_counters().await;
        if self.dump {
            ready.sender().send(true);
            loop {
                let signal = signal_receiver.receive().await;
                if let Signal::Terminate = signal {
                    self.publish_counters().await;
                    return Ok(());
                }
            }
        }

        // The client opens its stream here, where the process is already
        // running - never when its args were built.
        let server: logs_capnp::logs_args::server::Client = self.ctx.program_args.server_as()?;
        let response = server.open_stream_request().send().promise.await?;
        let stream = response.get()?.get_stream()?;
        ::tracing::info!(pid = self.ctx.pid, "logs stream opened");
        ready.sender().send(true);

        let dusk_client = dusk_core::local_client(self.ctx.namespace.clone()).await;
        let mut reader = self.buffer.reader(self.start_position, dusk_client).await?;
        let streamer = streamer::Streamer::<8>::new(128, 1024);
        let span = ::tracing::info_span!("logs_stream", pid = self.ctx.pid);
        let streaming = streamer
            .stream(&mut reader, &stream, self.minimum_severity, self.follow)
            .instrument(span);
        let terminated = async {
            loop {
                if let Signal::Terminate = signal_receiver.receive().await {
                    return;
                }
            }
        };
        let streaming_result = match select(streaming, terminated).await {
            Either::First(streaming_result) => streaming_result,
            Either::Second(()) => Ok(()),
        };
        self.streamer_done.signal(());
        self.publish_counters().await;
        Ok(streaming_result?)
    }
}

#[derive(dusk_program_proc::Portal)]
pub struct Portal {
    pub process: Process,
}

#[dusk_program_proc::impl_portal_rpc_server]
impl Portal {}

impl dusk_program_sh::sh_capnp::output_portal::Server for Portal {
    fn output(
        &mut self,
        params: dusk_program_sh::sh_capnp::output_portal::OutputParams,
        mut results: dusk_program_sh::sh_capnp::output_portal::OutputResults,
    ) -> Promise<(), ::capnp::Error> {
        dusk_capnp::pry!(results.set_pipeline());
        let stream = dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_stream());
        let process = self.process.clone();
        Promise::from_future(async move {
            let dump_result = async {
                if !process.dump {
                    process.streamer_done.wait().await;
                    return Ok(());
                }
                let dusk_client = dusk_core::local_client(process.ctx.namespace.clone()).await;
                let mut reader = process
                    .buffer
                    .reader(process.start_position, dusk_client)
                    .await?;
                let span = ::tracing::info_span!("logs_dump", pid = process.ctx.pid);
                dump::dump(
                    &mut reader,
                    &stream,
                    process.minimum_severity,
                    process.follow,
                )
                .instrument(span)
                .await?;
                Ok::<(), capnp::Error>(())
            }
            .await;
            results.get().set_daemonize(false);
            dump_result
        })
    }
}
