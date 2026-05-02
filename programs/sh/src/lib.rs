#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;
extern crate capnp;

use alloc::rc::Rc;
use core::cell::RefCell;
use alloc::sync::Arc;

use anyhow::Context;
use dusk_capnp::pry;
#[cfg(feature = "client")]
use dusk_program::IntoCapnp;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::embassy_sync::mutex::Mutex;
use dusk_program::ready::Ready;
use dusk_program::signal::SignalReceiver;
use dusk_program::stream::NoopStream;
use hashbrown::HashMap;
#[cfg(feature = "client")]
pub use linkme;

#[cfg(feature = "client")]
pub mod entry;

#[cfg(feature = "client")]
pub mod parser;

#[cfg(feature = "client")]
mod client;

mod interpreter;

use interpreter::{FunctionTable, Interpreter};

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("sh", VERSION, sh_capnp::PROGRAM_ID);

#[cfg(feature = "client")]
#[derive(dusk_program_proc::Args)]
pub struct ShArgs<S: entry::ShEntriesBuilder> {
    pub client: dusk::Client,
    pub options: capnp::message::Builder<capnp::message::HeapAllocator>,
    pub sh_entries_builder: S,
}

#[cfg(feature = "client")]
#[dusk_program_proc::impl_args_rpc_server]
impl<S: entry::ShEntriesBuilder> ShArgs<S> {
    fn get(
        &mut self,
        _params: sh_capnp::sh_args::GetParams,
        mut results: sh_capnp::sh_args::GetResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        results.get().set_client(self.client.clone());
        let opts_reader = pry!(
            self.options
                .get_root_as_reader::<sh_capnp::sh_options::Reader>()
        );
        pry!(results.get().set_options(opts_reader));
        capnp::capability::Promise::ok(())
    }

    fn build_program_args(
        &mut self,
        params: sh_capnp::sh_args::BuildProgramArgsParams,
        mut results: sh_capnp::sh_args::BuildProgramArgsResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        let command = pry!(pry!(pry!(params.get()).get_command()).to_str());
        let (remaining, words) = pry!(
            crate::parser::command_words(command)
                .map_err(|_| anyhow::anyhow!("invalid command `{}`", command))
                .into_capnp()
        );
        if !remaining.trim().is_empty() {
            return capnp::capability::Promise::err(capnp::Error::failed(format!(
                "invalid command `{command}`"
            )));
        }
        let program = pry!(
            words
                .first()
                .ok_or_else(|| anyhow::anyhow!("empty command"))
                .into_capnp()
        );
        let args = &words[1..];
        for entry in self.sh_entries_builder.get_entries() {
            if entry.info.name == *program {
                let pa = pry!(
                    entry
                        .program_args_builder
                        .build(self.client.clone(), args)
                        .context("program args builder failed")
                        .into_capnp()
                );
                results.get().set_program_args(pa);
                return capnp::capability::Promise::ok(());
            }
        }
        capnp::capability::Promise::err(capnp::Error::failed(format!(
            "no sh entry found for `{program}`"
        )))
    }
}

#[derive(dusk_program_proc::Launcher)]
pub struct Launcher {
    function_table: FunctionTable,
}

impl Launcher {
    pub fn new() -> Self {
        Self {
            function_table: Arc::new(Mutex::<CriticalSectionRawMutex, _>::new(HashMap::new())),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl dusk_program::launcher::LauncherMixin for Launcher {
    async fn launch(
        &mut self,
        process_context: ProcessContext,
    ) -> anyhow::Result<Box<dyn dusk_program::process::Process>> {
        Ok(Box::new(
            Process::with_context_and_function_table(process_context, self.function_table.clone())
                .await?,
        ))
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
    function_table: FunctionTable,
    state: Rc<RefCell<State>>,
}
impl Process {
    async fn with_context_and_function_table(
        ctx: ProcessContext,
        function_table: FunctionTable,
    ) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        Ok(Process {
            ctx,
            function_table: function_table,
            state: Rc::new(RefCell::new(State {
                interpreter: None,
                args_get_reply: None,
            })),
        })
    }
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    async fn with_context(ctx: ProcessContext) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        Ok(Self::with_context_and_function_table(ctx, Arc::new(Mutex::<CriticalSectionRawMutex, _>::new(HashMap::new()))).await?)
        
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
        let reply = get_reply.get()?;
        let client = reply.get_client()?;
        let options = reply.get_options()?;

        let name_suffix = match options.which()? {
            sh_capnp::sh_options::Which::DetachedScript(_) => "detached",
            sh_capnp::sh_options::Which::Script(_) => "script",
            sh_capnp::sh_options::Which::Server(_) => "server",
        };

        self.ctx
            .name
            .lock(|n| *n.borrow_mut() = Some(format!("sh[{name_suffix}]")));
        {
            self.state.borrow_mut().interpreter = Some(Interpreter::new(
                client,
                program_args.clone(),
                self.function_table.clone(),
            ));

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

    fn functions(
        &mut self,
        _params: sh_capnp::sh_portal::FunctionsParams,
        mut results: sh_capnp::sh_portal::FunctionsResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        let function_table = self.process.function_table.clone();
        Promise::from_future(async move {
            let symbols: alloc::vec::Vec<alloc::string::String> =
                function_table.lock().await.keys().cloned().collect();
            let mut list = results.get().init_symbols(symbols.len() as u32);
            for (i, name) in symbols.iter().enumerate() {
                list.set(i as u32, name.as_str());
            }
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
                    // The script has already been executed in `main` against a
                    // discard sink. Daemonize by returning without calling
                    // `done` on the caller's stream — the caller treats a
                    // missing `done` as "the process intends to keep running"
                    // and skips the kill.
                }
                sh_capnp::sh_options::Which::Server(()) => {
                    let mut request = stream.send_request();
                    let value_builder = request.get().init_value();
                    Value::Text("running in server mode".to_string())
                        .write_to_builder(value_builder)?;
                    request.send().await?;
                }
            }
            Ok(())
        })
    }
}
