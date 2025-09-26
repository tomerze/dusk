#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;

use std::rc::Rc;

#[allow(unused)]
#[prelude_import]
use dusk_capnp::prelude::*;

use alloc::format;
use capnp::capability::{FromClientHook, Promise};
use dusk_capnp::dusk_capnp::{dusk, portal};
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
        let program_args = self.process.program_args.clone();
        Promise::from_future(async move {
            let get_reply = program_args.get_request().send().promise.await?;
            let client = get_reply.get()?.get_client()?;
            let _options = get_reply.get()?.get_options()?;
            let ps_reply = client.ps_request().send().promise.await?;
            let process_entries = ps_reply.get()?.get_process_entries()?;

            for entry in process_entries.iter() {
                // TODO make this a table with value fields
                let pid = entry.get_pid();
                let process_client = entry.get_process()?;
                let program_id_reply = process_client.program_id_request().send().promise.await?;
                let program_id = program_id_reply.get()?.get_result();
                let line = format!("pid: `{pid}`, program_id: `{program_id}`");

                let mut send_request = stream.send_request();
                send_request.get().init_value().set_text(&line);
                send_request.send().await?;
            }

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
