#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;

#[allow(unused)]
#[prelude_import]
use dusk_capnp::prelude::*;

use capnp::capability::FromClientHook;
use dusk_capnp::dusk_capnp::portal;
use dusk_program::{basic_launcher, basic_process, portal::Portal};
use dusk_program_sh::program_args_builder::{ProgramArgsBuilder, StaticProgramArgsBuilderEntry};
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
    _program_args: ps_capnp::ps_args::Client,
}

impl PsPortal {
    pub fn new(_program_args: ps_capnp::ps_args::Client) -> Self {
        PsPortal { _program_args }
    }
}

impl Portal for PsPortal {}
impl portal::Server for PsPortal {}

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
        program_id: ps_capnp::PROGRAM_ID,
        name: "ps",
        builder: Box::new(PsProgramArgsBuilder {}),
    }
}
