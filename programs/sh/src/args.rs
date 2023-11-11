use crate::sh_capnp::sh_args;
use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::program_args;

pub struct ShArgs {}

impl program_args::Server for ShArgs {
    fn program_id(
        &mut self,
        _: program_args::ProgramIdParams,
        _: program_args::ProgramIdResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        Promise::ok(())
    }
}

impl sh_args::Server for ShArgs {}
