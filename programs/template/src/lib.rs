#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;
extern crate capnp;

#[cfg(feature = "client")]
pub mod client;

use {{program-name}}_capnp as program_capnp;
use dusk_capnp::capnp::capability::Promise;
use dusk_program::process::{DynamicReceiver, signal};
use dusk_program::process::signal::Signal;
use dusk_program::portal;

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("{{program-name}}", VERSION, program_capnp::PROGRAM_ID);

#[derive(dusk_program_proc::Args)]
#[cfg(feature = "client")]
pub struct Args {
    pub client: dusk::Client,
}

#[dusk_program_proc::impl_args_rpc_server]
#[cfg(feature = "client")]
impl Args {
    fn get(
        &mut self,
        _params: program_capnp::{{program-name}}_args::GetParams,
        mut results: program_capnp::{{program-name}}_args::GetResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        results.get().set_client(self.client.clone());
        results.get().init_options();
        Promise::ok(())
    }
}

#[derive(dusk_program_proc::Launcher)]
pub struct Launcher;

impl dusk_program::launcher::LauncherMixin for Launcher {
    fn launch(
        &mut self,
        pid: u64,
        namespace: alloc::rc::Rc<dusk_program::namespace::Namespace>,
        program_args: dusk_capnp::dusk_capnp::program_args::Client,
    ) -> anyhow::Result<Box<dyn dusk_program::process::Process>> {
        Ok(Box::new(Process::with_context(ProcessContext {
            pid,
            namespace,
            program_args,
        })))
    }
}

#[derive(Clone, dusk_program_proc::Process)]
pub struct Process {
    #[process_context]
    pub ctx: ProcessContext,
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    fn with_context(ctx: dusk_program::process::ProcessContext) -> Self
    where
        Self: Sized,
    {
        Process { ctx }
    }

    fn portal(&self) -> portal::Client {
        let client: program_capnp::{{program-name}}_portal::Client =
            capnp_rpc::new_client(Portal {
                process: self.clone(),
            });
        client.cast_to::<portal::Client>()
    }

    async fn main(
        &self,
        signal_receiver: DynamicReceiver<'async_trait, signal::Signal>,
    ) -> anyhow::Result<()> {
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
        let _process = self.process.clone();
        Promise::from_future(async move {
            // TODO: implement program logic here

            stream.done_request().send().promise.await?;
            Ok(())
        })
    }
}
