#![allow(internal_features)]
#![feature(prelude_import)]

extern crate alloc;
extern crate capnp;

use alloc::rc::Rc;
use core::cell::Cell;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::embassy_sync::watch::Watch;
use dusk_program::{ready::Ready, signal::SignalReceiver};

type Terminated = Watch<CriticalSectionRawMutex, bool, 16>;

#[cfg(feature = "client")]
pub mod client;

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("nightfall", VERSION, nightfall_capnp::PROGRAM_ID);

pub mod provision_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/provision_capnp.rs"));
}

#[derive(dusk_program_proc::Args)]
pub struct Args {
    #[data]
    pub data: ArgsDataBuilder,
}

impl Args {
    pub fn new(address: &str, port: u16) -> Self {
        let mut data = ArgsDataBuilder::new_default();
        {
            let mut root = data.init_root();
            root.set_address(address);
            root.set_port(port);
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
    #[process_context]
    pub ctx: ProcessContext,
    terminated: Rc<Terminated>,
}

impl Process {
    pub async fn with_context(ctx: ProcessContext) -> anyhow::Result<Self> {
        Ok(Process {
            ctx,
            terminated: Rc::new(Terminated::new_with(false)),
        })
    }
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    fn portal(&self) -> dusk_capnp::dusk_capnp::portal::Client {
        let client: nightfall_capnp::nightfall_portal::Client = capnp_rpc::new_client(Portal {
            process: self.clone(),
        });
        client.cast_to::<dusk_capnp::dusk_capnp::portal::Client>()
    }

    async fn main(
        &self,
        signal_receiver: SignalReceiver<'async_trait>,
        ready: Ready,
    ) -> anyhow::Result<()> {
        let (address, port) = self
            .ctx
            .program_args
            .with_data::<nightfall_capnp::nightfall_args::data::Owned, _, _>(|data| {
                let address = data.get_address()?.to_string()?;
                let port = data.get_port();
                Ok((address, port))
            })?;
        let ip_address: std::net::IpAddr = address.parse()?;
        let listener = async_io::Async::<std::net::TcpListener>::bind(std::net::SocketAddr::new(
            ip_address, port,
        ))?;
        tracing::info!(address = %address, port, "listening for sessions");
        let local_address = listener.get_ref().local_addr()?;
        let listen = if local_address.ip().is_unspecified() {
            alloc::format!(":{}", local_address.port())
        } else {
            local_address.to_string()
        };
        self.ctx
            .name
            .lock(|name| *name.borrow_mut() = Some(alloc::format!("nightfall[listen {listen}]")));

        ready.sender().send(true);

        loop {
            futures::select! {
                accept_result = listener.accept().fuse() => {
                    let (stream, _) = match accept_result {
                        Ok(accepted) => accepted,
                        Err(error) => {
                            tracing::error!(error = %error, "couldn't accept a connection");
                            continue;
                        }
                    };
                    if let Err(error) = stream.get_ref().set_nodelay(true) {
                        tracing::error!(error = %error, "couldn't set nodelay on a connection");
                        continue;
                    }
                    let (reader, writer) = stream.split();

                    let task_id = Rc::new(Cell::new(0));
                    let session_task = match dusk_core::session(
                        task_id.clone(),
                        self.namespace().clone(),
                        Box::pin(reader),
                        Box::pin(writer),
                    ) {
                        Ok(session_task) => session_task,
                        Err(error) => {
                            tracing::error!(
                                error = %error,
                                "couldn't take a connection: every session slot is in use"
                            );
                            continue;
                        }
                    };
                    task_id.set(session_task.id());
                    self.ctx.namespace.spawner.spawn(session_task);
                }
                signal = signal_receiver.receive().fuse() => {
                    if let Signal::Terminate = signal {
                        self.terminated.sender().send(true);
                        return Ok(());
                    }
                }
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
        _params: dusk_program_sh::sh_capnp::output_portal::OutputParams,
        mut results: dusk_program_sh::sh_capnp::output_portal::OutputResults,
    ) -> Promise<(), ::capnp::Error> {
        dusk_capnp::pry!(results.set_pipeline());
        let terminated = self.process.terminated.clone();
        Promise::from_future(async move {
            let mut receiver = terminated.receiver().ok_or_else(|| {
                capnp::Error::overloaded("every waiter slot for termination is in use".to_string())
            })?;
            while !receiver.get().await {
                receiver.changed().await;
            }
            results.get().set_daemonize(false);
            Ok(())
        })
    }
}
