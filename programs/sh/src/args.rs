// All of the code here actually runs on the client side, it's magic! (gated by the "client" feature)

use crate::sh_capnp;
use crate::sh_capnp::engine;

pub struct ShArgs {
    pub engine: engine::Client,
}

impl dusk_capnp::dusk_capnp::program_args::Server for ShArgs {
    fn program_id(
        &mut self,
        _params: dusk_capnp::dusk_capnp::program_args::ProgramIdParams,
        mut results: dusk_capnp::dusk_capnp::program_args::ProgramIdResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        results.get().set_program_id(crate::sh_capnp::PROGRAM_ID);
        capnp::capability::Promise::ok(())
    }
}

impl sh_capnp::sh_args::Server for ShArgs {
    fn get(
        &mut self,
        _params: sh_capnp::sh_args::GetParams,
        mut results: sh_capnp::sh_args::GetResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        results.get().set_engine(self.engine.clone());

        capnp::capability::Promise::ok(())
    }
}
