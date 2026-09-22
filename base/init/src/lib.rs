#![allow(internal_features)]
#![feature(prelude_import)]

extern crate alloc;
extern crate capnp;

use dusk_program::anyhow::Context;
use dusk_program::{ready::Ready, signal::SignalReceiver};
use dusk_program_sh::entry::StaticShEntriesBuilder;

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("init", VERSION, init_capnp::PROGRAM_ID);

// Hashed in const context, so the node carries the ids and never the names.
const VERSION_KEY: u64 = dusk_program_kvs_internal::key_id("dusk.version");
const GIT_REV_KEY: u64 = dusk_program_kvs_internal::key_id("dusk.git_rev");
const NAMESPACE_ID_KEY: u64 = dusk_program_kvs_internal::key_id("dusk.namespace_id");

#[cfg(feature = "client")]
mod known_keys {
    dusk_program_kvs_internal::known_key!(VERSION, "dusk.version");
    dusk_program_kvs_internal::known_key!(GIT_REV, "dusk.git_rev");
    dusk_program_kvs_internal::known_key!(NAMESPACE_ID, "dusk.namespace_id");
}

#[derive(dusk_program_proc::Args)]
pub struct Args {
    #[data]
    pub data: ArgsDataBuilder,
}

impl Args {
    pub fn new(init_script: &[u8]) -> anyhow::Result<Self> {
        let message = dusk_program_sh::bytecode::read(init_script)
            .context("the init script's bytecode is not a Script message")?;
        let mut data = ArgsDataBuilder::new_default();
        data.init_root().set_init_script(message.get_root()?)?;
        Ok(Args { data })
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
    #[process_context]
    pub ctx: ProcessContext,
}

impl Process {
    pub async fn with_context(ctx: ProcessContext) -> anyhow::Result<Self> {
        Ok(Process { ctx })
    }
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    fn portal(&self) -> dusk_capnp::dusk_capnp::portal::Client {
        let client: init_capnp::init_portal::Client = capnp_rpc::new_client(Portal {
            process: self.clone(),
        });
        client.cast_to::<dusk_capnp::dusk_capnp::portal::Client>()
    }

    async fn main(
        &self,
        signal_receiver: SignalReceiver<'async_trait>,
        ready: Ready,
    ) -> anyhow::Result<()> {
        let namespace_id = self.ctx.namespace.id;
        let kvs = dusk_program_kvs_internal::get_kvs(namespace_id);
        kvs.set(
            VERSION_KEY,
            Value::String(String::from(dusk_capnp::VERSION)),
        )
        .await;
        kvs.set(
            GIT_REV_KEY,
            Value::String(String::from(dusk_capnp::GIT_REV)),
        )
        .await;
        kvs.set(NAMESPACE_ID_KEY, Value::Uint(namespace_id)).await;

        ready.sender().send(true);

        let dusk_client = dusk_core::local_client(self.namespace().clone()).await;
        let programs_response = dusk_client.programs_request().send().promise.await?;
        let program_entries = programs_response.get()?.get_program_entries()?;
        for entry in program_entries.iter() {
            tracing::info!(
                program_id = entry.get_program_id(),
                version = entry.get_version()?.to_str()?,
                git_rev = entry.get_git_revision()?.to_str()?,
                "available program",
            );
        }

        let init_script = self
            .ctx
            .program_args
            .with_data::<init_capnp::init_args::data::Owned, _, _>(|data| {
                let mut message =
                    capnp::message::Builder::new(capnp::message::HeapAllocator::new());
                message.set_root(data.get_init_script()?)?;
                Ok(capnp::serialize::write_message_to_words(&message))
            })?;
        let sh_args = dusk_program_sh::ShArgs::new(
            dusk_client.clone(),
            StaticShEntriesBuilder::default(),
            dusk_program_sh::ShMode::DetachedScript(init_script),
        )?
        .as_program_args()?;

        let mut process_request = dusk_client.process_request();
        sh_args.with_reader(|reader| process_request.get().set_program_args(reader))?;
        let process_response = process_request.send().promise.await?;
        let mut run_request = dusk_client.run_request();
        run_request
            .get()
            .set_process(process_response.get()?.get_result()?);
        run_request.send().promise.await?;

        loop {
            if let Signal::Terminate = signal_receiver.receive().await {
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
impl Portal {}
