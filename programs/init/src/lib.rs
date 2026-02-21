#![allow(internal_features)]
#![feature(prelude_import)]

extern crate alloc;
extern crate capnp;

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("init", VERSION, init_capnp::PROGRAM_ID);

#[derive(dusk_program_proc::Args)]
pub struct Args {
    address: std::string::String,
    port: u16,
}

impl Args {
    pub fn new(address: &str, port: u16) -> Self {
        Args {
            address: address.to_string(),
            port,
        }
    }
}

#[dusk_program_proc::impl_args_rpc_server]
impl Args {
    fn get(
        &mut self,
        _params: init_capnp::init_args::GetParams,
        mut results: init_capnp::init_args::GetResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        let mut options = results.get().init_options();
        options.set_address(&self.address);
        options.set_port(self.port);

        Promise::ok(())
    }
}

#[derive(dusk_program_proc::Launcher)]
pub struct Launcher;

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
        signal_receiver: DynamicReceiver<'async_trait, signal::Signal>,
    ) -> anyhow::Result<()> {
        let program_args = self
            .ctx
            .program_args
            .clone()
            .cast_to::<init_capnp::init_args::Client>();

        let get_reply = program_args.get_request().send().promise.await?;
        let options = get_reply.get()?.get_options()?;
        let address = options.get_address()?;
        let port = options.get_port();
        let listener =
            async_net::TcpListener::bind(format!("{}:{}", address.to_str()?, port)).await?;

        loop {
            futures::select! {
                accept_result = listener.accept().fuse() => {
                    let (stream, _) = accept_result?;
                    stream.set_nodelay(true)?;
                    let (reader, writer) = stream.split();
                    let session_task = dusk_core::session(self.namespace().clone(), Box::pin(reader), Box::pin(writer));
                    self.ctx.namespace.spawner
                        .spawn(session_task)
                        .map_err(|err| anyhow::anyhow!("failed to spawn session task {err:#?}"))?;
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
