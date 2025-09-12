use crate::sh_capnp::sh_args;
use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::program_args;

pub struct ShArgs {}

impl program_args::Server for ShArgs {
    fn program_id(
        &mut self,
        _params: program_args::ProgramIdParams,
        mut results: program_args::ProgramIdResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        results.get().set_program_id(crate::sh_capnp::PROGRAM_ID);
        Promise::ok(())
    }
}

impl sh_args::Server for ShArgs {}
