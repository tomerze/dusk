#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

use alloc::rc::Rc;
use core::cell::{Cell, RefCell};

use dusk_program::{ready::Ready, signal::SignalReceiver};

extern crate alloc;
extern crate capnp;

#[cfg(feature = "client")]
pub mod client;

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("kvs", VERSION, kvs_capnp::PROGRAM_ID);

#[derive(dusk_program_proc::Args)]
pub struct Args {
    #[data]
    pub data: ArgsDataBuilder,
}

impl Args {
    pub fn get(key: &str) -> Self {
        let mut data = ArgsDataBuilder::new_default();
        data.init_root().set_get(key);
        Args { data }
    }

    pub fn set(key: &str, value: &Value) -> capnp::Result<Self> {
        let mut data = ArgsDataBuilder::new_default();
        {
            let mut set = data.init_root().init_set();
            set.set_key(key);
            value.write_to_builder(set.init_value())?;
        }
        Ok(Args { data })
    }

    pub fn delete(key: &str) -> Self {
        let mut data = ArgsDataBuilder::new_default();
        data.init_root().set_delete(key);
        Args { data }
    }

    pub fn exists(key: &str) -> Self {
        let mut data = ArgsDataBuilder::new_default();
        data.init_root().set_exists(key);
        Args { data }
    }

    pub fn bind() -> Self {
        let mut data = ArgsDataBuilder::new_default();
        data.init_root().set_bind(());
        Args { data }
    }
}

#[dusk_program_proc::impl_args_rpc_server]
impl Args {}

#[derive(dusk_program_proc::Launcher, Default)]
pub struct Launcher;

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
    /// What `output` streams; `None` streams nothing.
    result: Rc<RefCell<Option<Value>>>,
    bound: Rc<Cell<bool>>,
    #[process_context]
    pub ctx: ProcessContext,
}

impl Process {
    pub async fn with_context(ctx: dusk_program::process::ProcessContext) -> anyhow::Result<Self> {
        Ok(Process {
            result: Rc::new(RefCell::new(None)),
            bound: Rc::new(Cell::new(false)),
            ctx,
        })
    }
}

/// No store yet: these answer as an empty store would.
impl Process {
    async fn get(&self, _key: &str) -> Option<Value> {
        None
    }

    async fn set(&self, _key: &str, _value: Value) {}

    async fn delete(&self, _key: &str) -> bool {
        false
    }

    async fn exists(&self, _key: &str) -> bool {
        false
    }
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    fn portal(&self) -> portal::Client {
        let client: kvs_capnp::kvs_portal::Client = capnp_rpc::new_client(Portal {
            process: self.clone(),
        });
        client.cast_to::<portal::Client>()
    }

    async fn main(
        &self,
        signal_receiver: SignalReceiver<'async_trait>,
        ready: Ready,
    ) -> anyhow::Result<()> {
        let data = self
            .ctx
            .program_args
            .data_owned::<kvs_capnp::kvs_args::data::Owned>()?;
        match data.get_root_as_reader()?.which()? {
            kvs_capnp::kvs_args::data::Which::Get(key) => {
                let value = self.get(key?.to_str()?).await;
                *self.result.borrow_mut() = Some(value.unwrap_or(Value::Null));
            }
            kvs_capnp::kvs_args::data::Which::Set(set) => {
                let value = Value::from_reader(set.get_value()?)?;
                self.set(set.get_key()?.to_str()?, value).await;
            }
            kvs_capnp::kvs_args::data::Which::Delete(key) => {
                let deleted = self.delete(key?.to_str()?).await;
                *self.result.borrow_mut() = Some(Value::Bool(deleted));
            }
            kvs_capnp::kvs_args::data::Which::Exists(key) => {
                let exists = self.exists(key?.to_str()?).await;
                *self.result.borrow_mut() = Some(Value::Bool(exists));
            }
            kvs_capnp::kvs_args::data::Which::Bind(()) => self.bound.set(true),
        }

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
    fn get(
        &mut self,
        params: kvs_capnp::kvs_portal::GetParams,
        mut results: kvs_capnp::kvs_portal::GetResults,
    ) -> Promise<(), ::capnp::Error> {
        let key = String::from(dusk_capnp::pry!(
            dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_key()).to_str()
        ));
        let process = self.process.clone();
        Promise::from_future(async move {
            let value = process.get(&key).await.unwrap_or(Value::Null);
            value.write_to_builder(results.get().init_value())?;
            Ok(())
        })
    }

    fn set(
        &mut self,
        params: kvs_capnp::kvs_portal::SetParams,
        _results: kvs_capnp::kvs_portal::SetResults,
    ) -> Promise<(), ::capnp::Error> {
        let params = dusk_capnp::pry!(params.get());
        let key = String::from(dusk_capnp::pry!(
            dusk_capnp::pry!(params.get_key()).to_str()
        ));
        let value = dusk_capnp::pry!(Value::from_reader(dusk_capnp::pry!(params.get_value())));
        let process = self.process.clone();
        Promise::from_future(async move {
            process.set(&key, value).await;
            Ok(())
        })
    }

    fn delete(
        &mut self,
        params: kvs_capnp::kvs_portal::DeleteParams,
        mut results: kvs_capnp::kvs_portal::DeleteResults,
    ) -> Promise<(), ::capnp::Error> {
        let key = String::from(dusk_capnp::pry!(
            dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_key()).to_str()
        ));
        let process = self.process.clone();
        Promise::from_future(async move {
            results.get().set_deleted(process.delete(&key).await);
            Ok(())
        })
    }

    fn exists(
        &mut self,
        params: kvs_capnp::kvs_portal::ExistsParams,
        mut results: kvs_capnp::kvs_portal::ExistsResults,
    ) -> Promise<(), ::capnp::Error> {
        let key = String::from(dusk_capnp::pry!(
            dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_key()).to_str()
        ));
        let process = self.process.clone();
        Promise::from_future(async move {
            results.get().set_exists(process.exists(&key).await);
            Ok(())
        })
    }
}

impl dusk_program_sh::sh_capnp::output_portal::Server for Portal {
    fn output(
        &mut self,
        params: dusk_program_sh::sh_capnp::output_portal::OutputParams,
        mut results: dusk_program_sh::sh_capnp::output_portal::OutputResults,
    ) -> Promise<(), ::capnp::Error> {
        dusk_capnp::pry!(results.set_pipeline());
        let stream = dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_stream());
        if self.process.bound.get() {
            // Withholding `done` is the caller's signal that we daemonized.
            return Promise::ok(());
        }
        let result = self.process.result.borrow_mut().take();
        Promise::from_future(async move {
            if let Some(value) = result {
                let mut send_request = stream.send_request();
                let value_builder = send_request.get().init_value();
                value.write_to_builder(value_builder)?;
                send_request.send().await?;
            }
            stream.done_request().send().promise.await?;
            Ok(())
        })
    }
}
