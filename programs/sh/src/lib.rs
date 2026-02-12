#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;
extern crate capnp;

#[cfg(feature = "client")]
pub use linkme;

use dusk_program::dusk_capnp::dusk_capnp::process;
use dusk_program::dusk_capnp::pry;
use dusk_program::stream::UndoneStream;

#[cfg(feature = "client")]
pub mod entry;

#[cfg(feature = "client")]
pub mod engine;

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("sh", VERSION, sh_capnp::PROGRAM_ID);

#[derive(dusk_program_proc::Args)]
#[cfg(feature = "client")]
pub struct ShArgs {
    pub engine: sh_capnp::engine::Client,
}

#[dusk_program_proc::impl_args_rpc_server]
#[cfg(feature = "client")]
impl ShArgs {
    fn get(
        &mut self,
        _params: sh_capnp::sh_args::GetParams,
        mut results: sh_capnp::sh_args::GetResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        results.get().set_engine(self.engine.clone());

        capnp::capability::Promise::ok(())
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
    fn with_context(ctx: ProcessContext) -> Self
    where
        Self: Sized,
    {
        Process { ctx }
    }
    fn portal(&self) -> portal::Client {
        let client: sh_capnp::sh_portal::Client = capnp_rpc::new_client(Portal {
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

impl Portal {
    async fn command_string_to_program_args(
        engine: sh_capnp::engine::Client,
        command: &str,
    ) -> anyhow::Result<dusk_capnp::dusk_capnp::program_args::Client> {
        let mut build_from_string_request = engine.build_program_args_from_string_request();
        build_from_string_request.get().set_string(command);
        let build_from_string_reply = build_from_string_request.send().promise.await?;
        let result = build_from_string_reply.get()?.get_program_args()?;
        Ok(result)
    }

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

    // TODO change this to pipe io and support input as well
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
        let command = pry!(pry!(pry!(params.get()).get_command()).to_string());
        let output = pry!(pry!(params.get()).get_output());

        let program_args = self.process.ctx.program_args.clone();

        Promise::from_future(async move {
            // TODO actually parse the command and make it work like a shell
            let program_args = capnp::capability::FromClientHook::cast_to::<
                sh_capnp::sh_args::Client,
            >(program_args);
            let engine = capnp_rpc::new_future_client(async move {
                let get_request_result = program_args.get_request().send().promise.await?;
                get_request_result.get()?.get_engine()
            });
            let engine_clone = engine.clone();
            let client = capnp_rpc::new_future_client(async move {
                let client_request = engine_clone.client_request().send().promise.await?;
                client_request.get()?.get_client()
            });

            let args = Self::command_string_to_program_args(engine, &command)
                .await
                .map_err(|e| capnp::Error::failed(e.to_string()))?;
            let process = Self::execute_program_args(client.clone(), args)
                .await
                .map_err(|e| capnp::Error::failed(e.to_string()))?;

            Self::portal_and_pipe_output(process.clone(), output.clone())
                .await
                .map_err(|e| capnp::Error::failed(e.to_string()))?;

            let mut kill_request = client.kill_request();
            kill_request.get().set_process(process);
            kill_request.get().set_signal(15); // SIGTERM
            kill_request.send().promise.await?;
            output.done_request().send().promise.await?;

            Ok(())
        })
    }
}
