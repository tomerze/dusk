#![allow(internal_features)]
#![feature(prelude_import)]
#![feature(min_specialization)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;
extern crate capnp;

#[cfg(feature = "client")]
pub mod client;

use dusk_program::dusk_capnp::dusk_capnp::dusk;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Args {
    pub client: dusk::Client,
}
pub struct Launcher;

#[derive(Clone, Default)]
pub struct ProcessState;

#[derive(Default)]
pub struct PortalState;

impl PortalState {
    async fn inner_ps(
        process: &PsProcess,
    ) -> capnp::Result<(Vec<u64>, Vec<u64>, Vec<String>, Vec<String>)> {
        let program_args = process.program_args.clone();

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

dusk_program_proc::definition! {
    metadata("ps", VERSION, ps_capnp::PROGRAM_ID)

    /// Interface of
    /// client <-> process
    [args]
    public_type: Args
    rpc_server: {
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

    /// Interface of
    /// process <-> process.
    [launcher]
    public_type: Launcher
    mixin: {}

    /// Interface of
    /// core -> process.
    [process]
    state_type: ProcessState
    mixin: {}

    /// Interface of
    /// client <-> portal
    [portal]
    state_type: PortalState
    rpc_server: {
        fn input(
            &mut self,
            _params: portal::InputParams,
            mut results: portal::InputResults,
        ) -> Promise<(), ::capnp::Error> {
            results.get().set_stream(capnp_rpc::new_client(
                dusk_program::stream::NoopStream::default(),
            ));
            Promise::ok(())
        }

        fn output(
            &mut self,
            params: portal::OutputParams,
            mut results: portal::OutputResults,
        ) -> Promise<(), ::capnp::Error> {
            dusk_capnp::pry!(results.set_pipeline());
            let stream = dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_stream());
            let process = self.process.clone();
            Promise::from_future(async move {
                let (pids, program_ids, names, versions) = <PsPortalState>::inner_ps(&process).await?;

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
}
