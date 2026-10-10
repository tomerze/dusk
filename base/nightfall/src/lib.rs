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

mod backoff;
#[cfg(feature = "client")]
pub mod client;
pub mod connect;
mod file;
mod identity;
mod link;
mod node_key;
mod provisioning;
mod tls;
mod tpm;

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

    pub fn connect(arguments: &connect::ConnectArgs) -> Self {
        let mut data = ArgsDataBuilder::new_default();
        {
            let mut root = data.init_root().init_connect();
            root.set_fleet(&arguments.fleet);
            root.set_fleet_server_name(arguments.fleet_server_name.as_deref().unwrap_or(""));
            root.set_provision(&arguments.provision);
            root.set_provision_server_name(
                arguments.provision_server_name.as_deref().unwrap_or(""),
            );
            root.set_trust_anchors(&arguments.trust_anchors);
            root.set_install_token_file(arguments.install_token_file.as_deref().unwrap_or(""));
            root.set_heartbeat_timeout_seconds(arguments.heartbeat_timeout_seconds);
        }
        Args { data }
    }
}

fn read_connect(
    reader: nightfall_capnp::nightfall_args::connect::Reader<'_>,
) -> capnp::Result<connect::ConnectArgs> {
    let optional = |text: alloc::string::String| (!text.is_empty()).then_some(text);
    Ok(connect::ConnectArgs {
        fleet: reader.get_fleet()?.to_string()?,
        fleet_server_name: optional(reader.get_fleet_server_name()?.to_string()?),
        provision: reader.get_provision()?.to_string()?,
        provision_server_name: optional(reader.get_provision_server_name()?.to_string()?),
        trust_anchors: reader.get_trust_anchors()?.to_string()?,
        install_token_file: optional(reader.get_install_token_file()?.to_string()?),
        heartbeat_timeout_seconds: reader.get_heartbeat_timeout_seconds(),
    })
}

enum Mode {
    Listen {
        address: alloc::string::String,
        port: u16,
    },
    Connect(connect::ConnectArgs),
}

#[dusk_program_proc::impl_args_rpc_server]
impl Args {}

#[derive(dusk_program_proc::Launcher)]
pub struct Launcher {
    tid: u64,
}

impl Default for Launcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Launcher {
    pub fn new() -> Self {
        let tid = dusk_core::driver::tid();
        dusk_program_kvs_internal::own_keys(tid, &identity::IDENTITY_KEYS);
        Self { tid }
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        dusk_program_kvs_internal::disown_keys(self.tid, &identity::IDENTITY_KEYS);
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
        let mode = self
            .ctx
            .program_args
            .with_data::<nightfall_capnp::nightfall_args::data::Owned, _, _>(|data| {
                if data.has_connect() {
                    return Ok(Mode::Connect(read_connect(data.get_connect()?)?));
                }
                Ok(Mode::Listen {
                    address: data.get_address()?.to_string()?,
                    port: data.get_port(),
                })
            })?;
        match mode {
            Mode::Listen { address, port } => {
                self.listen(&address, port, signal_receiver, ready).await
            }
            Mode::Connect(arguments) => self.connect(&arguments, signal_receiver, ready).await,
        }
    }
}

impl Process {
    async fn connect(
        &self,
        arguments: &connect::ConnectArgs,
        signal_receiver: SignalReceiver<'_>,
        ready: Ready,
    ) -> anyhow::Result<()> {
        let settings = connect::Settings::new(arguments)
            .map_err(|reason| anyhow::anyhow!("nightfall can't link to the fleet: {reason}"))?;
        let kvs = dusk_program_kvs_internal::get_kvs(self.namespace().id);
        anyhow::ensure!(
            kvs.keeps_persistent_keys().await,
            "nightfall can't link to the fleet: this node keeps no persistent kvs keys to hold its identity"
        );
        let fleet = settings.fleet.to_string();
        self.ctx
            .name
            .lock(|name| *name.borrow_mut() = Some(alloc::format!("nightfall[connect {fleet}]")));
        ready.sender().send(true);
        let terminate = async {
            loop {
                if let Signal::Terminate = signal_receiver.receive().await {
                    return;
                }
            }
        };
        dusk_program::embassy_futures::select::select(
            connect::run(settings, self.namespace().clone(), kvs),
            terminate,
        )
        .await;
        tracing::info!(fleet, "stopped linking to the fleet");
        self.terminated.sender().send(true);
        Ok(())
    }

    async fn listen(
        &self,
        address: &str,
        port: u16,
        signal_receiver: SignalReceiver<'_>,
        ready: Ready,
    ) -> anyhow::Result<()> {
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
