use capnp::capability::FromClientHook;
use dusk_capnp::dusk_capnp::portal;
use dusk_program::portal::Portal;
use dusk_program::{basic_launcher, basic_process, impl_program_args_server};

extern crate alloc;

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

pub struct PsArgs {}

impl_program_args_server!(PsArgs, crate::ps_capnp::PROGRAM_ID);

impl ps_capnp::ps_args::Server for PsArgs {}

#[derive(Default)]
pub struct PsPortal {}

impl Portal for PsPortal {}
impl portal::Server for PsPortal {}

impl ps_capnp::ps_portal::Server for PsPortal {}
