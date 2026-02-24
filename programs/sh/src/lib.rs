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
use dusk_program::dusk_capnp::dusk_capnp::process;
use dusk_program::dusk_capnp::pry;
use dusk_program::stream::UndoneStream;

#[cfg(feature = "client")]
pub mod entry;

#[cfg(feature = "client")]
pub mod compiler;

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
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    async fn with_context(ctx: ProcessContext) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        Ok(Process { ctx })
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

impl Portal {
    async fn execute_program_args(
        client: dusk_capnp::dusk_capnp::dusk::Client,
        program_args: dusk_capnp::dusk_capnp::program_args::Client,
    ) -> anyhow::Result<process::Client> {
        let mut process_request = client.process_request();
        process_request.get().set_program_args(program_args);
        let process = capnp_rpc::new_future_client(async move {
            let process_reply = process_request.send().promise.await?;
            let process = process_reply.get()?.get_result()?;
            let mut run_request = client.run_request();
            run_request.get().set_process(process.clone());
            let _run_reply = run_request.send().promise.await?;
            Ok(process)
        });

        Ok(process)
    }

    async fn portal_and_pipe_output(
        process: process::Client,
        output: dusk_capnp::dusk_capnp::stream::Client,
    ) -> anyhow::Result<()> {
        let portal: sh_capnp::output_portal::Client = capnp_rpc::new_future_client(async move {
            let portal_request = process.portal_request();
            let portal_reply = portal_request.send().promise.await?;
            Ok(portal_reply
                .get()?
                .get_result()?
                .cast_to::<sh_capnp::output_portal::Client>())
        });

        let (undone_stream, done_receiver) = UndoneStream::new_with_done_receiver(output);

        let mut output_request = portal.output_request();
        output_request
            .get()
            .set_stream(capnp_rpc::new_client(undone_stream));
        let _output_reply = output_request.send().promise.await?;
        done_receiver.await.map_err(|e| anyhow::anyhow!("{}", e))?;

        Ok(())
    }
}

#[dusk_program_proc::impl_portal_rpc_server]
impl Portal {
    fn sh(
        &mut self,
        params: sh_capnp::sh_portal::ShParams,
        _results: sh_capnp::sh_portal::ShResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        let script = pry!(pry!(params.get()).get_script());
        let output = pry!(pry!(params.get()).get_output());

        let args_to_execute = pry!(script.get_program_args());
        let background = script.get_background();

        let program_args = self.process.ctx.program_args.clone();

        Promise::from_future(async move {
            let program_args = capnp::capability::FromClientHook::cast_to::<
                sh_capnp::sh_args::Client,
            >(program_args);
            let client = capnp_rpc::new_future_client(async move {
                let get_request_result = program_args.get_request().send().promise.await?;
                get_request_result.get()?.get_client()
            });

            let process = Self::execute_program_args(client.clone(), args_to_execute)
                .await
                .context("process execution failed")
                .into_capnp()?;

            if background {
                // Since `process` is a future client we need to somehow trigger it's creation.
                let _pid = process.pid_request().send().promise.await?;
                output.done_request().send().promise.await?;
                return Ok(());
            }

            Self::portal_and_pipe_output(process.clone(), output.clone())
                .await
                .context("output streaming failed")
                .into_capnp()?;
            let pid = process
                .pid_request()
                .send()
                .promise
                .await?
                .get()?
                .get_result();
            let mut kill_request = client.kill_request();
            kill_request.get().set_pid(pid);
            kill_request.get().set_signal(15); // SIGTERM
            kill_request.send().promise.await?;
            output.done_request().send().promise.await?;

            Ok(())
        })
    }
}
