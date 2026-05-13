#![allow(internal_features)]
#![feature(prelude_import)]
#![feature(impl_trait_in_assoc_type)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;
extern crate capnp;

use alloc::rc::Rc;
use alloc::sync::Arc;
use core::cell::{Cell, RefCell};

use anyhow::Context;
use dusk_capnp::pry;
#[cfg(feature = "client")]
use dusk_program::IntoCapnp;
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

#[cfg(feature = "client")]
mod client;

mod interpreter;

use interpreter::{FunctionTable, Interpreter, Stop};

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("sh", VERSION, sh_capnp::PROGRAM_ID);

#[cfg(feature = "client")]
pub enum ShMode {
    Server,
    Script(alloc::string::String),
    DetachedScript(alloc::string::String),
}

#[cfg(feature = "client")]
#[derive(dusk_program_proc::Args)]
pub struct ShArgs<S: entry::ShEntriesBuilder> {
    #[data]
    pub data: capnp::message::TypedBuilder<sh_capnp::sh_args::data::Owned>,
    pub client: dusk::Client,
    pub sh_entries_builder: S,
}

#[cfg(feature = "client")]
impl<S: entry::ShEntriesBuilder> ShArgs<S> {
    pub fn new(client: dusk::Client, sh_entries_builder: S, mode: ShMode) -> anyhow::Result<Self> {
        let mut data =
            capnp::message::TypedBuilder::<sh_capnp::sh_args::data::Owned>::new_default();
        {
            let mut data_builder = data.init_root();
            match mode {
                ShMode::Server => data_builder.set_server(()),
                ShMode::Script(command) => {
                    let mut parser = crate::parser::Parser::new();
                    parser.parse(&command, data_builder.init_script())?;
                }
                ShMode::DetachedScript(command) => {
                    let mut parser = crate::parser::Parser::new();
                    parser.parse(&command, data_builder.init_detached_script())?;
                }
            }
        }
        Ok(Self {
            data,
            client,
            sh_entries_builder,
        })
    }
}

#[cfg(feature = "client")]
#[dusk_program_proc::impl_args_rpc_server]
impl<S: entry::ShEntriesBuilder> ShArgs<S> {
    fn build_program_args(
        &mut self,
        params: sh_capnp::sh_args::server::BuildProgramArgsParams,
        mut results: sh_capnp::sh_args::server::BuildProgramArgsResults,
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
                let program_args = pry!(
                    entry
                        .program_args_builder
                        .build(self.client.clone(), args)
                        .context("program args builder failed")
                        .into_capnp()
                );
                pry!(program_args.with_reader(|reader| results.get().set_program_args(reader)));
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

struct State {
    interpreter: Option<Interpreter>,
    active_stops: alloc::vec::Vec<Rc<Stop>>,
}

#[embassy_executor::task(pool_size = 16)]
async fn sh_exec_task(
    task_id: Rc<Cell<u32>>,
    pid: u64,
    interpreter: Interpreter,
    script_msg: capnp::message::Builder<capnp::message::HeapAllocator>,
    output: dusk_capnp::dusk_capnp::stream::Client,
    stop: Rc<Stop>,
    state: Rc<RefCell<State>>,
    completion: Rc<
        dusk_program::embassy_sync::signal::Signal<CriticalSectionRawMutex, anyhow::Result<()>>,
    >,
) {
    use tracing::Instrument;
    let span = tracing::info_span!("sh_exec", task_id = task_id.get(), pid);
    async move {
        let result: anyhow::Result<()> = async {
            let reader = script_msg.get_root_as_reader::<sh_capnp::script::Reader>()?;
            interpreter.exec(reader, output, &stop).await
        }
        .await;
        state
            .borrow_mut()
            .active_stops
            .retain(|s| !Rc::ptr_eq(s, &stop));
        completion.signal(result);
    }
    .instrument(span)
    .await;
}

fn spawn_sh_exec_task(
    ctx: &ProcessContext,
    interpreter: Interpreter,
    script: sh_capnp::script::Reader<'_>,
    output: dusk_capnp::dusk_capnp::stream::Client,
    state: Rc<RefCell<State>>,
    stop: Rc<Stop>,
) -> capnp::Result<
    Rc<dusk_program::embassy_sync::signal::Signal<CriticalSectionRawMutex, anyhow::Result<()>>>,
> {
    let mut script_msg = capnp::message::Builder::new_default();
    script_msg.set_root::<sh_capnp::script::Owned>(script)?;

    state.borrow_mut().active_stops.push(stop.clone());

    let completion: Rc<
        dusk_program::embassy_sync::signal::Signal<CriticalSectionRawMutex, anyhow::Result<()>>,
    > = Rc::new(dusk_program::embassy_sync::signal::Signal::new());

    let task_id = Rc::new(Cell::new(0u32));
    let token = sh_exec_task(
        task_id.clone(),
        ctx.pid,
        interpreter,
        script_msg,
        output,
        stop,
        state,
        completion.clone(),
    )
    .map_err(|e| capnp::Error::failed(format!("failed to spawn sh exec task: {e:?}")))?;
    task_id.set(token.id());
    ctx.namespace.spawner.spawn(token);
    Ok(completion)
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
            function_table,
            state: Rc::new(RefCell::new(State {
                interpreter: None,
                active_stops: alloc::vec::Vec::new(),
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
        Ok(Self::with_context_and_function_table(
            ctx,
            Arc::new(Mutex::<CriticalSectionRawMutex, _>::new(HashMap::new())),
        )
        .await?)
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
        let sh_args_client = self
            .ctx
            .program_args
            .server_as::<sh_capnp::sh_args::server::Client>()?;
        let client = dusk_core::local_client(self.namespace().clone()).await;

        let (name_suffix, is_detached) = self
            .ctx
            .program_args
            .with_data::<sh_capnp::sh_args::data::Owned, _, _>(|data| {
                Ok(match data.which()? {
                    sh_capnp::sh_args::data::Which::Server(_) => ("server", false),
                    sh_capnp::sh_args::data::Which::Script(_) => ("script", false),
                    sh_capnp::sh_args::data::Which::DetachedScript(_) => ("detached", true),
                })
            })?;
        self.ctx
            .name
            .lock(|n| *n.borrow_mut() = Some(format!("sh[{name_suffix}]")));

        self.state.borrow_mut().interpreter = Some(Interpreter::new(
            client,
            sh_args_client,
            self.function_table.clone(),
        ));

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
            match signal_receiver.receive().await {
                Signal::Terminate => {
                    for stop in self.state.borrow().active_stops.iter() {
                        stop.signal(());
                    }
                    return Ok(());
                }
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
        let params = pry!(params.get());
        let script = pry!(params.get_script());
        let output = pry!(params.get_output());
        let stop_client = pry!(params.get_stop());

        let stop = Rc::new(Stop::new());
        let completion = pry!(spawn_sh_exec_task(
            &self.process.ctx,
            interpreter,
            script,
            output,
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
            let data = ctx
                .program_args
                .data_owned::<sh_capnp::sh_args::data::Owned>()?;
            match data.get_root_as_reader()?.which()? {
                sh_capnp::sh_args::data::Which::Server(_) => {
                    let mut request = stream.send_request();
                    let value_builder = request.get().init_value();
                    Value::Text("running in server mode".to_string())
                        .write_to_builder(value_builder)?;
                    request.send().await?;
                }
                sh_capnp::sh_args::data::Which::Script(script) => {
                    let interpreter = state_cell.borrow().interpreter.as_ref().unwrap().clone();
                    let completion = spawn_sh_exec_task(
                        &ctx,
                        interpreter,
                        script?,
                        stream.clone(),
                        state_cell.clone(),
                        Rc::new(Stop::new()),
                    )?;
                    completion
                        .wait()
                        .await
                        .map_err(|error| capnp::Error::failed(format!("{error:?}")))?;
                }
                sh_capnp::sh_args::data::Which::DetachedScript(_) => {
                    // Already ran in `main` against a discard sink.
                    // Daemonize by returning without calling `done` on the
                    // caller's stream — the caller treats a missing `done`
                    // as "the process intends to keep running" and skips
                    // the kill.
                }
            }
            Ok(())
        })
    }
}
