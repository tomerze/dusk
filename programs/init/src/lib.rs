#![allow(internal_features)]
#![feature(prelude_import)]

extern crate alloc;

extern crate capnp;

const VERSION: &str = env!("CARGO_PKG_VERSION");

// TODO: remove when Args becomes a derive-after-definition like Portal
#[derive(dusk_program_proc::Args)]
pub struct Args {
    address: std::string::String,
    port: u16,
}

pub type InitArgs = Args;

impl Args {
    pub fn new(address: &str, port: u16) -> Self {
        Args {
            address: address.to_string(),
            port,
        }
    }
}

pub struct Launcher;

#[derive(Clone, Default)]
pub struct ProcessState;

dusk_program_proc::definition! {
    metadata("init", VERSION, init_capnp::PROGRAM_ID)

    [launcher]
    public_type: Launcher
    mixin: {}

    [process]
    state_type: ProcessState
    mixin: {
        async fn main(
            &self,
            signal_receiver: DynamicReceiver<'async_trait, signal::Signal>,
        ) -> anyhow::Result<()> {
            let program_args = self
                .program_args
                .clone()
                .cast_to::<init_capnp::init_args::Client>();
            let get_reply = program_args.get_request().send().promise.await?;
            let options = get_reply.get()?.get_options()?;
            let address = options.get_address()?;
            let port = options.get_port();
            let listener = async_net::TcpListener::bind(format!("{}:{}", address.to_str()?, port)).await?;

            loop {
                futures::select! {
                    accept_result = listener.accept().fuse() => {
                        let (stream, _) = accept_result?;
                        stream.set_nodelay(true)?;
                        let (reader, writer) = stream.split();
                        let session_task = dusk_core::session(self.namespace.clone(), Box::pin(reader), Box::pin(writer));
                        let spawner = unsafe { Spawner::for_current_executor().await };
                        spawner
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
}

// --- Portal (must be after definition! which generates InitProcess and __derive_portal) ---

#[derive(dusk_program_proc::Portal)]
pub struct Portal {
    _process: InitProcess,
}

impl Portal {
    pub fn new(_process: InitProcess) -> Self {
        Portal { _process }
    }
}

#[dusk_program_proc::args_rpc_server]
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

#[dusk_program_proc::portal_rpc_server]
impl Portal {}
