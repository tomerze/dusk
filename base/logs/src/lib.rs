#![allow(internal_features)]
#![feature(prelude_import)]
#![feature(impl_trait_in_assoc_type)]
#![cfg_attr(not(any(feature = "client", test)), no_std)]

use dusk_program::{ready::Ready, signal::SignalReceiver};
use tracing::Instrument;

extern crate alloc;
extern crate capnp;
extern crate self as dusk_program_logs;

#[macro_use]
extern crate std;

mod buffer;
mod config;
mod layer;
mod stream;

pub use buffer::{DropCounts, LogBuffer, Reader, StartPosition, Writer};
pub use config::{LaneConfig, LogsConfig};
pub use layer::BufferLayer;

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("logs", VERSION, logs_capnp::PROGRAM_ID);

/// OTel/OTLP log record schema, compiled from `capnp/log_record.capnp`.
pub mod log_record_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/log_record_capnp.rs"));
}

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
mod viewer;

#[derive(dusk_program_proc::Launcher)]
pub struct Launcher {
    pub buffer: LogBuffer,
}

impl Launcher {
    /// Builds the buffer; with the `console` feature this also installs the
    /// global tracing subscriber (buffer capture plus console output at INFO),
    /// unless one is already installed.
    pub fn new(config: LogsConfig) -> anyhow::Result<Self> {
        let buffer = LogBuffer::new(config)?;
        #[cfg(feature = "console")]
        layer::bootstrap(buffer.clone());
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
    pub buffer: LogBuffer,
    #[process_context]
    pub ctx: ProcessContext,
}

impl Process {
    async fn with_context_and_buffer(
        ctx: ProcessContext,
        buffer: LogBuffer,
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
                let server: logs_capnp::logs_args::server::Client =
                    process.ctx.program_args.server_as()?;
                let minimum_severity = process
                    .ctx
                    .program_args
                    .with_data::<logs_capnp::logs_args::data::Owned, _, _>(|data| {
                        // Unknown ordinals (a newer client) act as "no floor".
                        Ok(data.get_level().map(|level| level as u16).unwrap_or(0))
                    })?;
                let dusk_client = dusk_core::local_client(process.ctx.namespace.clone()).await;
                let mut reader =
                    process.buffer.reader(StartPosition::Replay, dusk_client).await?;
                let span = tracing::info_span!("logs_stream", pid = process.ctx.pid);
                let streaming = stream::stream(&mut reader, &server, minimum_severity);
                if let Err(error) = streaming.instrument(span).await {
                    tracing::warn!(error = %error, "the logs stream failed");
                }
                Ok::<(), capnp::Error>(())
            }
            .await;
            stream.done_request().send().promise.await?;
            stream_result
        })
    }
}
