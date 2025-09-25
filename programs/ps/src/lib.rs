#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;

#[allow(unused)]
#[prelude_import]
use dusk_capnp::prelude::*;

use capnp::capability::{FromClientHook, Promise};
use dusk_program::{
    basic_launcher, basic_process, impl_portal_server, portal::Portal, stream::NoopStream,
};
use dusk_program_sh::program_args_builder::{
    ProgramArgsBuilder, ProgramInfo, StaticProgramArgsBuilderEntry,
};

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
pub struct PsArgs {}

#[cfg(feature = "client")]
impl_program_args_server!(PsArgs, crate::ps_capnp::PROGRAM_ID);

#[cfg(feature = "client")]
impl ps_capnp::ps_args::Server for PsArgs {}

pub struct PsPortal {
    _process: PsProcess,
}

impl PsPortal {
    pub fn new(_process: PsProcess) -> Self {
        PsPortal { _process }
    }
}

impl Portal for PsPortal {}

impl_portal_server!(PsPortal);

impl ps_capnp::ps_portal::Server for PsPortal {}

struct PsProgramArgsBuilder {}

impl ProgramArgsBuilder for PsProgramArgsBuilder {
    fn build_from_string(
        &self,
        _s: &str,
    ) -> anyhow::Result<dusk_capnp::dusk_capnp::program_args::Client> {
        let client: ps_capnp::ps_args::Client = capnp_rpc::new_client(PsArgs {});
        Ok(client.cast_to::<dusk_capnp::dusk_capnp::program_args::Client>())
    }
}

#[distributed_slice(dusk_program_sh::program_args_builder::SH_PROGRAM_ARGS_BUILDERS)]
pub fn program_args_builder_entry() -> StaticProgramArgsBuilderEntry {
    StaticProgramArgsBuilderEntry {
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
        builder: Box::new(PsProgramArgsBuilder {}),
    }
}
