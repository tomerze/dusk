#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;

use std::rc::Rc;
use std::vec;

#[allow(unused)]
#[prelude_import]
use dusk_capnp::prelude::*;

use alloc::vec::Vec;
use capnp::capability::{FromClientHook, Promise};
use dusk_capnp::dusk_capnp::{dusk, portal};
use dusk_capnp::value::{Field, Value};
use dusk_program::{basic_launcher, basic_process, portal::Portal};
use dusk_program_sh::entry::{ProgramArgsBuilder, ProgramInfo, ShEntry};

use linkme::distributed_slice;

#[cfg(feature = "client")]
use dusk_program::impl_program_args_server;

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

    async fn inner_ps(process: &PsProcess) -> capnp::Result<(Vec<u64>, Vec<u64>)> {
        let program_args = process.program_args.clone();

        let get_reply = program_args.get_request().send().promise.await?;
        let client = get_reply.get()?.get_client()?;
        let _options = get_reply.get()?.get_options()?;
        let ps_reply = client.ps_request().send().promise.await?;
        let process_entries = ps_reply.get()?.get_process_entries()?;

        let mut pids = vec![];
        let mut program_ids = vec![];

        for entry in process_entries.iter() {
            pids.push(entry.get_pid());

            let process = entry.get_process()?;
            let program_id_reply = process.program_id_request().send().promise.await?;
            program_ids.push(program_id_reply.get()?.get_result());
        }

        Ok((pids, program_ids))
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
            let (pids, program_ids) = Self::inner_ps(&process).await?;

            let mut send_request = stream.send_request();

            let value_builder = send_request.get().init_value();
            Value::Fields(vec![
                Field {
                    key: "pid".to_string(),
                    value: Value::List(pids.iter().map(|pid| Value::Uint(*pid)).collect()),
                },
                Field {
                    key: "program_id".to_string(),
                    value: Value::List(program_ids.iter().map(|id| Value::Uint(*id)).collect()),
                },
            ])
            .write_to_builder(value_builder)?;

            send_request.send().await?;
            stream.done_request().send().promise.await?;
            Ok(())
        })
    }
}

impl ps_capnp::ps_portal::Server for PsPortal {}

struct PsProgramArgsBuilder {}

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
