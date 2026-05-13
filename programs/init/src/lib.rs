#![allow(internal_features)]
#![feature(prelude_import)]

extern crate alloc;
extern crate capnp;

use alloc::rc::Rc;
use core::cell::Cell;
use dusk_program::{ready::Ready, signal::SignalReceiver};

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("init", VERSION, init_capnp::PROGRAM_ID);

#[derive(dusk_program_proc::Args)]
pub struct Args {
    #[data]
    pub data: capnp::message::TypedBuilder<init_capnp::init_args::data::Owned>,
}

impl Args {
    pub fn new(address: &str, port: u16) -> Self {
        let mut data =
            capnp::message::TypedBuilder::<init_capnp::init_args::data::Owned>::new_default();
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

#[derive(dusk_program_proc::Launcher)]
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
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    async fn with_context(ctx: ProcessContext) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        Ok(Process { ctx })
    }
    fn portal(&self) -> dusk_capnp::dusk_capnp::portal::Client {
        let client: init_capnp::init_portal::Client = capnp_rpc::new_client(Portal {
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
            .with_data::<init_capnp::init_args::data::Owned, _, _>(|data| {
                let address = data.get_address()?.to_string()?;
                let port = data.get_port();
                Ok((address, port))
            })?;
        let listener = async_net::TcpListener::bind(format!("{}:{}", address, port)).await?;
        ready.sender().send(true);

        loop {
            futures::select! {
                accept_result = listener.accept().fuse() => {
                    let (stream, _) = accept_result?;
                    stream.set_nodelay(true)?;
                    let (reader, writer) = stream.split();

                    let task_id = Rc::new(Cell::new(0));
                    let session_task = dusk_core::session(
                        task_id.clone(),
                        self.namespace().clone(),
                        Box::pin(reader),
                        Box::pin(writer),
                    )?;
                    task_id.set(session_task.id());
                    self.ctx.namespace.spawner.spawn(session_task);
                }
                signal = signal_receiver.receive().fuse() => {
                    match signal {
                        Signal::Terminate => return Ok(()),
                        Signal::Unknown(_signal) => {}
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
