#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;
extern crate capnp;

use alloc::rc::Rc;
use core::cell::RefCell;

use anyhow::Context;
use dusk_capnp::pry;
use dusk_program::ready::Ready;
use dusk_program::signal::SignalReceiver;
use dusk_program::stream::NoopStream;
#[cfg(feature = "client")]
pub use linkme;

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
    pub options: capnp_rpc::ImbuedMessageBuilder<capnp::message::HeapAllocator>,
}

#[dusk_program_proc::impl_args_rpc_server]
impl ShArgs {
    fn get(
        &mut self,
        _params: sh_capnp::sh_args::GetParams,
        mut results: sh_capnp::sh_args::GetResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        results.get().set_client(self.client.clone());
        let opts_builder = pry!(self.options.get_root::<sh_capnp::sh_options::Builder>());
        pry!(results.get().set_options(opts_builder.reborrow_as_reader()));
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

type ArgsGetReply = capnp::capability::Response<sh_capnp::sh_args::get_results::Owned>;

struct State {
    interpreter: Option<Interpreter>,
    args_get_reply: Option<ArgsGetReply>,
}

#[derive(Clone, dusk_program_proc::Process)]
pub struct Process {
    #[process_context]
    pub ctx: ProcessContext,
    state: Rc<RefCell<State>>,
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    async fn with_context(ctx: ProcessContext) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        Ok(Process {
            ctx,
            state: Rc::new(RefCell::new(State {
                interpreter: None,
                args_get_reply: None,
            })),
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
        let program_args = capnp::capability::FromClientHook::cast_to::<sh_capnp::sh_args::Client>(
            self.ctx.program_args.clone(),
        );
        let get_reply = program_args.get_request().send().promise.await?;
        {
            let reply = get_reply.get()?;
            let client = reply.get_client()?;
            let options = reply.get_options()?;

            self.state.borrow_mut().interpreter = Some(Interpreter::new(client));

            if let sh_capnp::sh_options::Which::DetachedScript(script) = options.which()? {
                let interpreter = self.state.borrow().interpreter.as_ref().unwrap().clone();
                let noop: dusk_capnp::dusk_capnp::stream::Client =
                    capnp_rpc::new_client(NoopStream::new());
                interpreter.exec(script?, noop).await?;
            }
        }
        self.state.borrow_mut().args_get_reply = Some(get_reply);

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

#[dusk_program_proc::impl_portal_rpc_server]
impl Portal {
    fn sh(
        &mut self,
        params: sh_capnp::sh_portal::ShParams,
        _results: sh_capnp::sh_portal::ShResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        let interpreter = self
            .process
            .state
            .borrow()
            .interpreter
            .as_ref()
            .unwrap()
            .clone();
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

impl sh_capnp::output_portal::Server for Portal {
    fn output(
        &mut self,
        params: sh_capnp::output_portal::OutputParams,
        mut results: sh_capnp::output_portal::OutputResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        tracing::info!("output called!");
        pry!(results.set_pipeline());
        let stream = pry!(pry!(params.get()).get_stream());
        let get_reply = pry!(
            self.process
                .state
                .borrow_mut()
                .args_get_reply
                .take()
                .ok_or_else(|| { capnp::Error::failed("args reply not available".into()) })
        );
        let state_cell = self.process.state.clone();
        Promise::from_future(async move {
            let reply = get_reply.get()?;
            let options = reply.get_options()?;
            match options.which()? {
                sh_capnp::sh_options::Which::Script(script) => {
                    let interpreter = state_cell.borrow().interpreter.as_ref().unwrap().clone();
                    interpreter
                        .exec(script?, stream)
                        .await
                        .context("script execution failed")
                        .into_capnp()?;
                }
                sh_capnp::sh_options::Which::DetachedScript(_) => {
                    stream.done_request().send().promise.await?;
                }
                sh_capnp::sh_options::Which::Server(()) => {
                    return Err(capnp::Error::failed("running in server mode".into()));
                }
            }
            Ok(())
        })
    }
}
