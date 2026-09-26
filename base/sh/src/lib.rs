#![allow(internal_features)]
#![allow(clippy::too_many_arguments)]
#![feature(prelude_import)]
#![feature(impl_trait_in_assoc_type)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;
extern crate capnp;
extern crate self as dusk_program_sh;

use alloc::rc::Rc;
use alloc::sync::Arc;
use core::cell::RefCell;

use dusk_capnp::pry;
use dusk_program::embassy_futures::select::{Either, select};
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

mod exec;
mod interpreter;

use exec::{State, spawn_sh_exec_task};
use interpreter::{FunctionTable, Interpreter, Stop};

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("sh", VERSION, sh_capnp::PROGRAM_ID);

mod args;
pub use args::{ShArgs, ShMode};
#[allow(clippy::all)]
pub mod bytecode_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/bytecode_capnp.rs"));
}
pub use bytecode_capnp::bytecode;

pub type BytecodeMessage =
    dusk_capnp::capnp_rpc::ImbuedMessageBuilder<capnp::message::HeapAllocator>;

#[cfg(feature = "client")]
pub mod client;

#[derive(dusk_program_proc::Launcher)]
pub struct Launcher;

impl Default for Launcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Launcher {
    pub fn new() -> Self {
        Self
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
    function_table: FunctionTable,
    state: Rc<RefCell<State>>,
}
impl Process {
    #[allow(clippy::arc_with_non_send_sync)]
    async fn with_context(ctx: ProcessContext) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        Ok(Process {
            ctx,
            function_table: Arc::new(Mutex::<CriticalSectionRawMutex, _>::new(HashMap::new())),
            state: Rc::new(RefCell::new(State {
                interpreter: None,
                active_stops: alloc::vec::Vec::new(),
            })),
        })
    }
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
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
        let client = dusk_core::local_client(self.namespace().clone()).await;

        let (name_suffix, is_detached) = self
            .ctx
            .program_args
            .with_data::<sh_capnp::sh_args::data::Owned, _, _>(|data| {
                Ok(match data.which()? {
                    sh_capnp::sh_args::data::Which::Server(_) => (String::from("server"), false),
                    sh_capnp::sh_args::data::Which::Script(_) => (String::from("script"), false),
                    sh_capnp::sh_args::data::Which::DetachedScript(_) => {
                        (String::from("detached"), true)
                    }
                    sh_capnp::sh_args::data::Which::Prompt(client_hostname) => (
                        format!("prompt \u{27f7} {}", client_hostname?.to_str()?),
                        false,
                    ),
                })
            })?;
        self.ctx
            .name
            .lock(|n| *n.borrow_mut() = Some(format!("sh[{name_suffix}]")));

        self.state.borrow_mut().interpreter =
            Some(Interpreter::new(client, self.function_table.clone()));

        if is_detached {
            let interpreter = self.state.borrow().interpreter.as_ref().unwrap().clone();
            let noop: dusk_capnp::dusk_capnp::stream::Client =
                capnp_rpc::new_client(NoopStream::new());
            self.ctx
                .program_args
                .with_data::<sh_capnp::sh_args::data::Owned, _, _>(|data| {
                    if let sh_capnp::sh_args::data::Which::DetachedScript(script) = data.which()? {
                        spawn_sh_exec_task(
                            &self.ctx,
                            interpreter.clone(),
                            script?,
                            noop.clone(),
                            self.state.clone(),
                            Rc::new(Stop::new()),
                        )?;
                    }
                    Ok(())
                })?;
        }
        ready.sender().send(true);

        loop {
            if let Signal::Terminate = signal_receiver.receive().await {
                for stop in self.state.borrow().active_stops.iter() {
                    stop.signal(());
                }
                return Ok(());
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
        let params = pry!(params.get());
        let script = pry!(params.get_script());
        let output = pry!(params.get_output());
        let stop_client = pry!(params.get_stop());

        let stop = Rc::new(Stop::new());
        let completion = pry!(spawn_sh_exec_task(
            &self.process.ctx,
            interpreter,
            script,
            output.clone(),
            self.process.state.clone(),
            stop.clone(),
        ));
        Promise::from_future(async move {
            let listen = async {
                let _ = stop_client.stop_request().send().promise.await;
                stop.signal(());
                core::future::pending::<()>().await
            };
            let result = match select(completion.wait(), listen).await {
                Either::First(result) => result,
                Either::Second(()) => unreachable!(),
            };
            if let Err(error) = output.done_request().send().promise.await {
                tracing::warn!(error = %error, "failed to close the caller's stream");
            }
            result.map_err(|error| capnp::Error::failed(format!("{error:?}")))
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
        let state_cell = self.process.state.clone();
        let ctx = self.process.ctx.clone();
        Promise::from_future(async move {
            let mut data = ctx
                .program_args
                .data_owned::<sh_capnp::sh_args::data::Owned>()?;
            match data
                .get_root::<sh_capnp::sh_args::data::Builder>()?
                .into_reader()
                .which()?
            {
                sh_capnp::sh_args::data::Which::Server(_) => {
                    let mut request = stream.send_request();
                    let value_builder = request.get().init_value();
                    Value::Text("running in server mode".to_string())
                        .write_to_builder(value_builder)?;
                    request.send().await?;
                    results.get().set_daemonize(true);
                }
                sh_capnp::sh_args::data::Which::Prompt(_) => {
                    let stop = Rc::new(Stop::new());
                    state_cell.borrow_mut().active_stops.push(stop.clone());
                    stop.wait().await;
                    state_cell
                        .borrow_mut()
                        .active_stops
                        .retain(|active| !Rc::ptr_eq(active, &stop));
                    results.get().set_daemonize(false);
                }
                sh_capnp::sh_args::data::Which::Script(script) => {
                    let interpreter = state_cell.borrow().interpreter.as_ref().unwrap().clone();
                    let completion = spawn_sh_exec_task(
                        &ctx,
                        interpreter,
                        script?,
                        stream,
                        state_cell.clone(),
                        Rc::new(Stop::new()),
                    )?;
                    completion
                        .wait()
                        .await
                        .map_err(|error| capnp::Error::failed(format!("{error:?}")))?;
                    results.get().set_daemonize(false);
                }
                sh_capnp::sh_args::data::Which::DetachedScript(_) => {
                    results.get().set_daemonize(true);
                }
            }
            Ok(())
        })
    }
}
