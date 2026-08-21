#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

use alloc::rc::Rc;
use core::cell::RefCell;

use dusk_program::{ready::Ready, signal::SignalReceiver};

extern crate alloc;
extern crate capnp;

#[cfg(feature = "client")]
pub mod client;

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("programs", VERSION, programs_capnp::PROGRAM_ID);

#[cfg(feature = "client")]
const UNRESOLVED_NAME: &str = "N/A";

#[cfg(feature = "client")]
const SHELL_ENTRY_SEPARATOR: &str = " | ";

#[cfg(feature = "client")]
#[derive(dusk_program_proc::Args)]
pub struct Args {
    #[data]
    pub data: ArgsDataBuilder,
    pub program_names: Vec<(u64, String)>,
}

#[cfg(feature = "client")]
impl Args {
    pub fn new(program_names: Vec<(u64, String)>) -> Self {
        let mut data = ArgsDataBuilder::new_default();
        data.init_root();
        Args {
            data,
            program_names,
        }
    }
}

#[cfg(feature = "client")]
#[dusk_program_proc::impl_args_rpc_server]
impl Args {
    fn transpose(
        &mut self,
        params: programs_capnp::programs_args::server::TransposeParams,
        _results: programs_capnp::programs_args::server::TransposeResults,
    ) -> Promise<(), ::capnp::Error> {
        let parameters = dusk_capnp::pry!(params.get());
        let program_ids = dusk_capnp::pry!(parameters.get_program_ids());
        let versions = dusk_capnp::pry!(parameters.get_versions());
        let git_revisions = dusk_capnp::pry!(parameters.get_git_revisions());
        let output = dusk_capnp::pry!(parameters.get_output());

        let names = program_ids
            .iter()
            .map(|program_id| {
                let entry_names: Vec<&str> = self
                    .program_names
                    .iter()
                    .filter(|(known_program_id, _)| *known_program_id == program_id)
                    .map(|(_, name)| name.as_str())
                    .collect();
                Value::String(if entry_names.is_empty() {
                    String::from(UNRESOLVED_NAME)
                } else {
                    entry_names.join(SHELL_ENTRY_SEPARATOR)
                })
            })
            .collect();
        let versions = dusk_capnp::pry!(
            versions
                .iter()
                .map(|version| Ok(Value::String(version?.to_string()?)))
                .collect::<::capnp::Result<Vec<_>>>()
        );
        let git_revisions = dusk_capnp::pry!(
            git_revisions
                .iter()
                .map(|git_revision| Ok(Value::String(git_revision?.to_string()?)))
                .collect::<::capnp::Result<Vec<_>>>()
        );
        let program_ids = program_ids.iter().map(Value::Uint).collect();

        let fields = Record::with_fields(
            programs_capnp::RESULT_TYPE_ID,
            [
                (b"Shell Entry".to_vec(), Value::List(names)),
                (b"Version On Node".to_vec(), Value::List(versions)),
                (b"Program ID".to_vec(), Value::List(program_ids)),
                (b"Git Revision".to_vec(), Value::List(git_revisions)),
            ],
        );

        Promise::from_future(async move {
            let mut send_request = output.send_request();
            Value::Record(fields).write_to_builder(send_request.get().init_value())?;
            send_request.send().await?;
            Ok(())
        })
    }
}

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

#[derive(Clone, Default)]
struct ProgramsResult {
    pub program_ids: Vec<u64>,
    pub versions: Vec<String>,
    pub git_revisions: Vec<String>,
}

#[derive(Clone, dusk_program_proc::Process)]
pub struct Process {
    result: Rc<RefCell<ProgramsResult>>,
    #[process_context]
    ctx: ProcessContext,
}

impl Process {
    pub async fn with_context(ctx: dusk_program::process::ProcessContext) -> anyhow::Result<Self> {
        Ok(Process {
            result: Rc::new(RefCell::new(ProgramsResult::default())),
            ctx,
        })
    }
}

#[async_trait::async_trait(?Send)]
impl dusk_program::process::ProcessMixin for Process {
    fn portal(&self) -> portal::Client {
        let client: programs_capnp::programs_portal::Client = capnp_rpc::new_client(Portal {
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

        let programs_reply = client.programs_request().send().promise.await?;
        let program_entries = programs_reply.get()?.get_program_entries()?;

        for entry in program_entries.iter() {
            let mut result = self.result.borrow_mut();
            result.program_ids.push(entry.get_program_id());
            result.versions.push(entry.get_version()?.to_string()?);
            result
                .git_revisions
                .push(entry.get_git_revision()?.to_string()?);
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
impl Portal {}

impl dusk_program_sh::sh_capnp::output_portal::Server for Portal {
    fn output(
        &mut self,
        params: dusk_program_sh::sh_capnp::output_portal::OutputParams,
        mut results: dusk_program_sh::sh_capnp::output_portal::OutputResults,
    ) -> Promise<(), ::capnp::Error> {
        dusk_capnp::pry!(results.set_pipeline());
        let stream = dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_stream());
        let process = self.process.clone();
        Promise::from_future(async move {
            let server: programs_capnp::programs_args::server::Client =
                process.ctx.program_args.server_as()?;

            let mut request = server.transpose_request();
            {
                let result = process.result.borrow();
                let mut builder = request.get();
                let mut program_ids = builder
                    .reborrow()
                    .init_program_ids(result.program_ids.len() as u32);
                for (index, program_id) in result.program_ids.iter().enumerate() {
                    program_ids.set(index as u32, *program_id);
                }
                let mut versions = builder
                    .reborrow()
                    .init_versions(result.versions.len() as u32);
                for (index, version) in result.versions.iter().enumerate() {
                    versions.set(index as u32, version.as_str());
                }
                let mut git_revisions = builder
                    .reborrow()
                    .init_git_revisions(result.git_revisions.len() as u32);
                for (index, git_revision) in result.git_revisions.iter().enumerate() {
                    git_revisions.set(index as u32, git_revision.as_str());
                }
                builder.set_output(stream.clone());
            }
            let transposed = request.send().promise.await;

            stream.done_request().send().promise.await?;
            transposed.map(|_| ())
        })
    }
}
