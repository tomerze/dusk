#![allow(internal_features)]
#![feature(prelude_import)]

use alloc::rc::Rc;

use dusk_program::{ready::Ready, signal::SignalReceiver};

extern crate alloc;
extern crate capnp;

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("sys", VERSION, sys_capnp::PROGRAM_ID);

#[derive(dusk_program_proc::Args)]
pub struct Args {
    #[data]
    pub data: ArgsDataBuilder,
}

impl Args {
    pub fn new(command: &str) -> Self {
        let mut data = ArgsDataBuilder::new_default();
        {
            let mut root = data.init_root();
            root.set_command(command);
        }
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
    output: Rc<
        embassy_sync::signal::Signal<
            embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
            capnp::Result<std::process::Output>,
        >,
    >,
    #[process_context]
    pub ctx: ProcessContext,
}

impl Process {
    pub async fn with_context(ctx: dusk_program::process::ProcessContext) -> anyhow::Result<Self> {
        Ok(Process {
            output: Rc::new(embassy_sync::signal::Signal::new()),
            ctx,
        })
    }
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    fn portal(&self) -> portal::Client {
        let client: sys_capnp::sys_portal::Client = capnp_rpc::new_client(Portal {
            process: self.clone(),
        });
        client.cast_to::<portal::Client>()
    }

    async fn main(
        &self,
        signal_receiver: SignalReceiver<'async_trait>,
        ready: Ready,
    ) -> anyhow::Result<()> {
        let command = self
            .ctx
            .program_args
            .with_data::<sys_capnp::sys_args::data::Owned, _, _>(|data| {
                Ok(alloc::string::String::from(data.get_command()?.to_str()?))
            })?;
        let child = duct::cmd("sh", ["-c", command.as_str()])
            .stdout_capture()
            .stderr_capture()
            .unchecked()
            .start()?;
        ready.sender().send(true);
        let output = blocking::unblock(move || child.into_output()).await;
        self.output
            .signal(output.map_err(|error| capnp::Error::failed(error.to_string())));
        loop {
            let signal = signal_receiver.receive().await;
            if let Signal::Terminate = signal {
                return Ok(());
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
