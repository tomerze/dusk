#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

use alloc::rc::Rc;
use core::cell::Cell;

use dusk_program::{ready::Ready, signal::SignalReceiver};

extern crate alloc;
extern crate capnp;

#[cfg(feature = "client")]
pub mod client;

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("date", VERSION, date_capnp::PROGRAM_ID);

#[derive(dusk_program_proc::Args)]
pub struct Args {
    #[data]
    pub data: ArgsDataBuilder,
}

impl Args {
    pub fn show() -> Self {
        let mut data = ArgsDataBuilder::new_default();
        data.init_root().set_show(());
        Args { data }
    }

    pub fn set_to(unix_time_ms: u64) -> Self {
        let mut data = ArgsDataBuilder::new_default();
        data.init_root().set_set_to(unix_time_ms);
        Args { data }
    }
}

#[dusk_program_proc::impl_args_rpc_server]
impl Args {}

#[derive(dusk_program_proc::Launcher, Default)]
pub struct Launcher;

impl Launcher {
    pub fn new() -> Self {
        Self
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
    formatted: Rc<Cell<Option<alloc::string::String>>>,
    #[process_context]
    pub ctx: ProcessContext,
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    async fn with_context(ctx: dusk_program::process::ProcessContext) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        Ok(Process {
            formatted: Rc::new(Cell::new(None)),
            ctx,
        })
    }

    fn portal(&self) -> portal::Client {
        let client: date_capnp::date_portal::Client = capnp_rpc::new_client(Portal {
            process: self.clone(),
        });
        client.cast_to::<portal::Client>()
    }

    async fn main(
        &self,
        signal_receiver: SignalReceiver<'async_trait>,
        ready: Ready,
    ) -> anyhow::Result<()> {
        let data = self
            .ctx
            .program_args
            .data_owned::<date_capnp::date_args::data::Owned>()?;
        let client = dusk_core::local_client(self.namespace().clone()).await;

        match data.get_root_as_reader()?.which()? {
            date_capnp::date_args::data::Which::Show(()) => {
                let reply = client.time_request().send().promise.await?;
                let unix_time_ms = reply.get()?.get_unix_time_ms();
                self.formatted.set(Some(format_unix_time_ms(unix_time_ms)));
            }
            date_capnp::date_args::data::Which::SetTo(unix_time_ms) => {
                let mut request = client.settime_request();
                request.get().set_unix_time_ms(unix_time_ms);
                request.send().promise.await?;
            }
        }

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
        let formatted = self.process.formatted.take();
        Promise::from_future(async move {
            if let Some(formatted) = formatted {
                let mut send_request = stream.send_request();
                let value_builder = send_request.get().init_value();
                Value::Text(formatted).write_to_builder(value_builder)?;
                send_request.send().await?;
            }
            stream.done_request().send().promise.await?;
            Ok(())
        })
    }
}

// Format a Unix-time-in-milliseconds value as `YYYY-MM-DD HH:MM:SS` (UTC).
// Date arithmetic is Howard Hinnant's `civil_from_days` algorithm
fn format_unix_time_ms(unix_time_ms: u64) -> alloc::string::String {
    let secs = unix_time_ms / 1000;
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let hour = tod / 3600;
    let minute = (tod / 60) % 60;
    let second = tod % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };

    alloc::format!("`{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}` (UTC)")
}
