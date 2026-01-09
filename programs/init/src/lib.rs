#![allow(internal_features)]
#![feature(prelude_import)]

extern crate alloc;
extern crate capnp; // Needed for ::capnp:: paths in macros

#[allow(unused)]
#[prelude_import]
use dusk_capnp::prelude::*;

use anyhow::{anyhow, Result};
use async_net::TcpListener;
use dusk_program::prelude::*;
use dusk_program::signal::Signal;
use dusk_program::{basic_launcher, portal::Portal};
use dusk_program::{impl_portal_server, impl_program_args_server, signal};

use std::string::String;

#[allow(clippy::all)]
pub mod init_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/init_capnp.rs"));
}

basic_launcher!(
    InitLauncher,
    init_capnp::PROGRAM_ID,
    InitProcess,
    init_capnp::init_args::Client
);

pub struct InitPortal {
    _process: InitProcess,
}

impl InitPortal {
    pub fn new(_process: InitProcess) -> Self {
        InitPortal { _process }
    }
}

impl Portal for InitPortal {}

impl_portal_server!(InitPortal);

impl init_capnp::init_portal::Server for InitPortal {}

pub struct InitArgs {
    address: String,
    port: u16,
}

impl InitArgs {
    pub fn new(address: &str, port: u16) -> Self {
        InitArgs {
            address: address.to_string(),
            port,
        }
    }
}

impl_program_args_server!(InitArgs, crate::init_capnp::PROGRAM_ID);

impl init_capnp::init_args::Server for InitArgs {
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

#[derive(Clone)]
pub struct InitProcess {
    pub pid: u64,
    pub namespace: alloc::rc::Rc<dusk_program::namespace::Namespace>,
    pub program_args: init_capnp::init_args::Client,
}

impl InitProcess {
    pub fn new(
        pid: u64,
        namespace: alloc::rc::Rc<dusk_program::namespace::Namespace>,
        program_args: init_capnp::init_args::Client,
    ) -> Self {
        InitProcess {
            pid,
            namespace,
            program_args,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::Process for InitProcess {
    fn pid(&self) -> u64 {
        self.pid
    }
    fn program_id(&self) -> u64 {
        init_capnp::PROGRAM_ID
    }
    fn name(&self) -> alloc::string::String {
        alloc::string::String::from("init")
    }
    fn version(&self) -> alloc::string::String {
        alloc::string::String::from(env!("CARGO_PKG_VERSION"))
    }
    fn namespace(&self) -> alloc::rc::Rc<dusk_program::namespace::Namespace> {
        self.namespace.clone()
    }
    fn clone_box(&self) -> Box<dyn dusk_program::process::Process> {
        Box::new(InitProcess {
            pid: self.pid,
            namespace: self.namespace.clone(),
            program_args: self.program_args.clone(),
        })
    }

    fn portal(&self) -> dusk_capnp::dusk_capnp::portal::Client {
        let client: init_capnp::init_portal::Client =
            capnp_rpc::new_client(InitPortal::new(self.clone()));
        client.cast_to::<dusk_capnp::dusk_capnp::portal::Client>()
    }

    async fn main(
        &self,
        signal_receiver: DynamicReceiver<'async_trait, signal::Signal>,
    ) -> Result<()> {
        let program_args = self
            .program_args
            .clone()
            .cast_to::<init_capnp::init_args::Client>();
        let get_reply = program_args.get_request().send().promise.await?;
        let options = get_reply.get()?.get_options()?;
        let address = options.get_address()?;
        let port = options.get_port();
        let listener = TcpListener::bind(format!("{}:{}", address.to_str()?, port)).await?;

        loop {
            futures::select! {
                accept_result = listener.accept().fuse() => {
                    let (stream, _) = accept_result?;
                    stream.set_nodelay(true)?;
                    let (reader, writer) = stream.split();
                    let session_task = dusk::session(self.namespace.clone(), Box::pin(reader), Box::pin(writer));
                    let spawner = unsafe { Spawner::for_current_executor().await };
                    spawner
                        .spawn(session_task)
                        .map_err(|err| anyhow!("failed to spawn session task {err:#?}"))?;
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
