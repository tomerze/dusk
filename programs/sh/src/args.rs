// All of the code here actually runs on the client side, it's magic! (gated by the "client" feature)

use crate::entry::ShEntriesBuilder;
use crate::{engine::ShEngine, sh_capnp};

pub struct ShArgs<S: ShEntriesBuilder> {
    pub engine: ShEngine<S>,
}

impl<S: ShEntriesBuilder> dusk_capnp::dusk_capnp::program_args::Server for ShArgs<S> {
    fn program_id(
        &mut self,
        _params: dusk_capnp::dusk_capnp::program_args::ProgramIdParams,
        mut results: dusk_capnp::dusk_capnp::program_args::ProgramIdResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        results.get().set_program_id(crate::sh_capnp::PROGRAM_ID);
        capnp::capability::Promise::ok(())
    }
}

impl<S: ShEntriesBuilder> sh_capnp::sh_args::Server for ShArgs<S> {
    fn get(
        &mut self,
        _params: sh_capnp::sh_args::GetParams,
        mut results: sh_capnp::sh_args::GetResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        let engine = capnp_rpc::new_client(self.engine.clone());
        results.get().set_engine(engine);
        results.get().set_client(self.engine.client.clone());

        capnp::capability::Promise::ok(())
    }
}
