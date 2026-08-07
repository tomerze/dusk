#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;
extern crate capnp;

use alloc::rc::Rc;
use dusk_program::embassy_futures::select::select;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::embassy_time::{Duration, Timer};
use dusk_program::{ready::Ready, signal::SignalReceiver};

#[cfg(feature = "client")]
pub mod client;

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("sleep", VERSION, sleep_capnp::PROGRAM_ID);

#[derive(dusk_program_proc::Args)]
pub struct Args {
    #[data]
    pub data: ArgsDataBuilder,
}

impl Args {
    pub fn duration_ms(milliseconds: u64) -> Self {
        let mut data = ArgsDataBuilder::new_default();
        data.init_root().set_duration_ms(milliseconds);
        Args { data }
    }
}

#[dusk_program_proc::impl_args_rpc_server]
impl Args {}

#[derive(dusk_program_proc::Launcher)]
pub struct Launcher;

impl Launcher {
    pub fn new() -> Self {
        Self
    }
}

impl Default for Launcher {
    fn default() -> Self {
        Self::new()
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
    terminated: Rc<dusk_program::embassy_sync::signal::Signal<CriticalSectionRawMutex, ()>>,
}

impl Process {
    pub async fn with_context(ctx: ProcessContext) -> anyhow::Result<Self> {
        Ok(Process {
            ctx,
            terminated: Rc::new(dusk_program::embassy_sync::signal::Signal::new()),
        })
    }
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    fn portal(&self) -> portal::Client {
        let client: sleep_capnp::sleep_portal::Client = capnp_rpc::new_client(Portal {
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
            match signal_receiver.receive().await {
                Signal::Terminate => {
                    self.terminated.signal(());
                    return Ok(());
                }
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
        let duration_ms = dusk_capnp::pry!(
            self.process
                .ctx
                .program_args
                .with_data::<sleep_capnp::sleep_args::data::Owned, _, _>(|data| {
                    Ok(data.get_duration_ms())
                })
        );
        let terminated = self.process.terminated.clone();
        Promise::from_future(async move {
            select(
                Timer::after(Duration::from_millis(duration_ms)),
                terminated.wait(),
            )
            .await;
            stream.done_request().send().promise.await?;
            Ok(())
        })
    }
}
