#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;
extern crate capnp;

use dusk_program::ready::Ready;
use dusk_program::signal::SignalReceiver;
#[cfg(feature = "client")]
pub use linkme;

use anyhow::Context;

#[cfg(feature = "client")]
pub mod entry;

#[cfg(feature = "client")]
pub mod compiler;

#[cfg(feature = "client")]
mod client;

mod interpreter;

use interpreter::Interpreter;

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("sh", VERSION, sh_capnp::PROGRAM_ID);

#[derive(dusk_program_proc::Args)]
pub struct ShArgs {
    pub client: dusk::Client,
}

#[dusk_program_proc::impl_args_rpc_server]
impl ShArgs {
    fn get(
        &mut self,
        _params: sh_capnp::sh_args::GetParams,
        mut results: sh_capnp::sh_args::GetResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        results.get().set_client(self.client.clone());
        results.get().init_options();

        capnp::capability::Promise::ok(())
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
    pub interpreter: Interpreter,
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    async fn with_context(ctx: ProcessContext) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let program_args = capnp::capability::FromClientHook::cast_to::<sh_capnp::sh_args::Client>(
            ctx.clone().program_args,
        );
        let client = capnp_rpc::new_future_client(async move {
            let get_request_result = program_args.get_request().send().promise.await?;
            get_request_result.get()?.get_client()
        });
        Ok(Process {
            ctx,
            interpreter: Interpreter::new(client),
        })
    }
    fn portal(&self) -> portal::Client {
        let client: sh_capnp::sh_portal::Client = capnp_rpc::new_client(Portal {
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

impl Portal {}

#[dusk_program_proc::impl_portal_rpc_server]
impl Portal {
    fn sh(
        &mut self,
        params: sh_capnp::sh_portal::ShParams,
        _results: sh_capnp::sh_portal::ShResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        let interpreter = self.process.interpreter.clone();
        Promise::from_future(async move {
            let params = params.get()?;
            let script = params.get_script()?;
            let output = params.get_output()?;

            interpreter
                .exec(script, output)
                .await
                .context("sh execution failed")
                .into_capnp()?;
            Ok(())
        })
    }
}
