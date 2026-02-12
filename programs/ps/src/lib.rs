#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;
extern crate capnp;

#[cfg(feature = "client")]
pub mod client;

const VERSION: &str = env!("CARGO_PKG_VERSION");

dusk_program_proc::metadata!("ps", VERSION, ps_capnp::PROGRAM_ID);

#[derive(dusk_program_proc::Args)]
#[cfg(feature = "client")]
pub struct Args {
    pub client: dusk::Client,
}

#[dusk_program_proc::impl_args_rpc_server]
#[cfg(feature = "client")]
impl Args {
    fn get(
        &mut self,
        _params: ps_capnp::ps_args::GetParams,
        mut results: ps_capnp::ps_args::GetResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        results.get().set_client(self.client.clone());
        results.get().init_options();
        Promise::ok(())
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
    fn with_context(ctx: dusk_program::process::ProcessContext) -> Self
    where
        Self: Sized,
    {
        Process { ctx }
    }

    fn portal(&self) -> portal::Client {
        let client: ps_capnp::ps_portal::Client = capnp_rpc::new_client(Portal {
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
    async fn inner_ps(
        process: &Process,
    ) -> capnp::Result<(Vec<u64>, Vec<u64>, Vec<String>, Vec<String>)> {
        let program_args = capnp::capability::FromClientHook::cast_to::<ps_capnp::ps_args::Client>(
            process.ctx.program_args.clone(),
        );

        let get_reply = program_args.get_request().send().promise.await?;
        let client = get_reply.get()?.get_client()?;
        let _options = get_reply.get()?.get_options()?;
        let ps_reply = client.ps_request().send().promise.await?;
        let process_entries = ps_reply.get()?.get_process_entries()?;

        let mut pids = vec![];
        let mut program_ids = vec![];
        let mut process_names = vec![];
        let mut program_versions = vec![];

        for entry in process_entries.iter() {
            pids.push(entry.get_pid());

            let process = entry.get_process()?;
            let program_id_reply = process.program_id_request().send().promise.await?;
            program_ids.push(program_id_reply.get()?.get_result());

            let name_reply = process.name_request().send().promise.await?;
            process_names.push(name_reply.get()?.get_result()?.to_string()?);

            let program_version_reply = process.version_request().send().promise.await?;
            program_versions.push(program_version_reply.get()?.get_result()?.to_string()?);
        }

        Ok((pids, program_ids, process_names, program_versions))
    }
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
            let (pids, program_ids, names, versions) = Portal::inner_ps(&process).await?;

            let mut send_request = stream.send_request();

            let pid_values = pids.iter().copied().map(Value::Uint).collect();
            let program_id_values = program_ids.iter().copied().map(Value::Uint).collect();
            let name_values = names.iter().cloned().map(Value::String).collect();
            let version_values = versions.iter().cloned().map(Value::String).collect();
            let fields = Record::with_fields(
                ps_capnp::RESULT_TYPE_ID,
                [
                    (b"name".to_vec(), Value::List(name_values)),
                    (b"version".to_vec(), Value::List(version_values)),
                    (b"pid".to_vec(), Value::List(pid_values)),
                    (b"program_id".to_vec(), Value::List(program_id_values)),
                ],
            );

            let value_builder = send_request.get().init_value();
            Value::Record(fields).write_to_builder(value_builder)?;

            send_request.send().await?;
            stream.done_request().send().promise.await?;
            Ok(())
        })
    }
}
