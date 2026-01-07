#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;

#[allow(unused)]
#[prelude_import]
use dusk_capnp::prelude::*;

use alloc::vec::Vec;
use capnp::capability::{FromClientHook, Promise};
use dusk_capnp::dusk_capnp::portal;
use dusk_program::value::{Record, Value};
use dusk_program::{basic_launcher, basic_process, portal::Portal};

#[cfg(feature = "client")]
use dusk_capnp::dusk_capnp::dusk;
#[cfg(feature = "client")]
use dusk_program::impl_program_args_server;
#[cfg(feature = "client")]
use dusk_program_sh::entry::{ProgramArgsBuilder, ProgramInfo, ShEntry};
#[cfg(feature = "client")]
use linkme::distributed_slice;
#[cfg(feature = "client")]
use std::rc::Rc;

#[allow(clippy::all)]
pub mod ps_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/ps_capnp.rs"));
}

basic_launcher!(
    PsLauncher,
    ps_capnp::PROGRAM_ID,
    PsProcess,
    ps_capnp::ps_args::Client
);
basic_process!(
    PsProcess,
    ps_capnp::PROGRAM_ID,
    "ps",
    env!("CARGO_PKG_VERSION"),
    PsPortal,
    ps_capnp::ps_portal::Client,
    ps_capnp::ps_args::Client
);

#[cfg(feature = "client")]
pub struct PsArgs {
    pub client: dusk::Client,
}

#[cfg(feature = "client")]
impl_program_args_server!(PsArgs, crate::ps_capnp::PROGRAM_ID);

#[cfg(feature = "client")]
impl ps_capnp::ps_args::Server for PsArgs {
    fn get(
        &mut self,
        _params: ps_capnp::ps_args::GetParams,
        mut results: ps_capnp::ps_args::GetResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        results.get().set_client(self.client.clone());
        // TODO set options for real
        results.get().init_options();
        Promise::ok(())
    }
}

pub struct PsPortal {
    process: PsProcess,
}

impl PsPortal {
    pub fn new(process: PsProcess) -> Self {
        PsPortal { process }
    }

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

impl Portal for PsPortal {}

impl portal::Server for PsPortal {
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
            let (pids, program_ids, names, versions) = Self::inner_ps(&process).await?;

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

impl ps_capnp::ps_portal::Server for PsPortal {}

#[cfg(feature = "client")]
struct PsProgramArgsBuilder {}

#[cfg(feature = "client")]
impl ProgramArgsBuilder for PsProgramArgsBuilder {
    fn build(
        &self,
        client: dusk::Client,
        _args: &str,
    ) -> anyhow::Result<dusk_capnp::dusk_capnp::program_args::Client> {
        let client: ps_capnp::ps_args::Client = capnp_rpc::new_client(PsArgs { client });
        Ok(client.cast_to::<dusk_capnp::dusk_capnp::program_args::Client>())
    }
}

#[cfg(feature = "client")]
#[distributed_slice(dusk_program_sh::entry::SH_ENTRIES)]
pub fn program_args_builder_entry() -> ShEntry {
    ShEntry {
        info: ProgramInfo {
            program_id: Some(ps_capnp::PROGRAM_ID),
            name: "ps",
            short_description: "list processes",
            long_description: r#"
The `ps` command is used to display information about the currently running processes.
* Use `ps` to list all currently running processes.
* Use `ps <pid>` to get more information about a specific process.
"#,
            version: env!("CARGO_PKG_VERSION"),
        },
        program_args_builder: Rc::new(PsProgramArgsBuilder {}),
    }
}
