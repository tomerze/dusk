use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::portal;
use dusk_program::portal::Portal;
use dusk_program::{basic_launcher, basic_process, impl_program_args_server};

extern crate alloc;

#[allow(clippy::all)]
pub mod sh_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/sh_capnp.rs"));
}

basic_launcher!(
    ShLauncher,
    sh_capnp::PROGRAM_ID,
    ShProcess,
    sh_capnp::sh_args::Client
);
basic_process!(
    ShProcess,
    sh_capnp::PROGRAM_ID,
    ShPortal,
    sh_capnp::sh_args::Client
);

pub struct ShArgs {}

impl_program_args_server!(ShArgs, crate::sh_capnp::PROGRAM_ID);

impl sh_capnp::sh_args::Server for ShArgs {}

#[derive(Default)]
pub struct ShPortal {}

impl Portal for ShPortal {}
impl portal::Server for ShPortal {}

impl sh_capnp::sh_portal::Server for ShPortal {
    fn sh(
        &mut self,
        _params: sh_capnp::sh_portal::ShParams,
        _results: sh_capnp::sh_portal::ShResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        Promise::ok(())
    }
}
