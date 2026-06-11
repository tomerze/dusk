#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(any(feature = "client", test)), no_std)]

use dusk_program::{ready::Ready, signal::SignalReceiver};

extern crate alloc;
extern crate capnp;
#[cfg(test)]
#[macro_use]
extern crate std;

#[cfg(feature = "client")]
pub mod client;

mod buffer;
mod config;
mod layer;

pub use buffer::{DropCounts, LogBuffer, LogEntry, Reader, StartPosition, Writer};
pub use config::{LaneConfig, LogsConfig};
pub use layer::BufferLayer;

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("logs", VERSION, logs_capnp::PROGRAM_ID);

/// OTel/OTLP log record schema, compiled from `capnp/log_record.capnp`.
pub mod log_record_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/log_record_capnp.rs"));
}

#[derive(dusk_program_proc::Args)]
pub struct Args {
    #[data]
    pub data: ArgsDataBuilder,
}

impl Args {
    pub fn new() -> Self {
        let mut data = ArgsDataBuilder::new_default();
        data.init_root();
        Args { data }
    }
}

impl Default for Args {
    fn default() -> Self {
        Self::new()
    }
}

#[dusk_program_proc::impl_args_rpc_server]
impl Args {}

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
        Ok(Box::new(Process::with_context(process_context).await?))
    }
}

#[derive(Clone, dusk_program_proc::Process)]
pub struct Process {
    #[process_context]
    pub ctx: ProcessContext,
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    async fn with_context(ctx: dusk_program::process::ProcessContext) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        Ok(Process { ctx })
    }

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
        Promise::from_future(async move {
            stream.done_request().send().promise.await?;
            Ok(())
        })
    }
}
