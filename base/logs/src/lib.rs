#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(any(feature = "client", test)), no_std)]

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
    #[process_context]
    pub ctx: ProcessContext,
}

impl Process {
    async fn with_context_and_buffer(
        ctx: ProcessContext,
        buffer: SignalBuffer,
    ) -> anyhow::Result<Self> {
        Ok(Process { ctx, buffer })
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
        ready.sender().send(true);
        loop {
            let signal = signal_receiver.receive().await;
            match signal {
                Signal::Terminate => return Ok(()),
                Signal::Unknown(_signal) => {}
            }
        }
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
            let stream_result = async {
                let (minimum_severity, start_position, follow, dump) =
                    process
                        .ctx
                        .program_args
                        .with_data::<logs_capnp::logs_args::data::Owned, _, _>(|data| {
                            let minimum_severity =
                                data.get_level().map(|level| level as u16).unwrap_or(0);
                            let flags = data.get_flags();
                            let start_position = if flags & FLAG_REPLAY != 0 {
                                StartPosition::Replay
                            } else {
                                StartPosition::Live
                            };
                            let follow = flags & FLAG_FOLLOW != 0;
                            let dump = flags & FLAG_DUMP != 0;
                            Ok((minimum_severity, start_position, follow, dump))
                        })?;
                let dusk_client = dusk_core::local_client(process.ctx.namespace.clone()).await;
                let mut reader = process.buffer.reader(start_position, dusk_client).await?;
                let span = ::tracing::info_span!("logs_stream", pid = process.ctx.pid);
                if dump {
                    dump::dump(&mut reader, &stream, minimum_severity, follow)
                        .instrument(span)
                        .await?;
                } else {
                    let server: logs_capnp::logs_args::server::Client =
                        process.ctx.program_args.server_as()?;
                    let streamer = streamer::Streamer::<8>::new(64, 256);
                    let streaming = streamer.stream(&mut reader, &server, minimum_severity, follow);
                    if let Err(error) = streaming.instrument(span).await {
                        ::tracing::warn!(error = %error, "the logs stream failed");
                    }
                }
                Ok::<(), capnp::Error>(())
            }
            .await;
            stream.done_request().send().promise.await?;
            stream_result
        })
    }
}
