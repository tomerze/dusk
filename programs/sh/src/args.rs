// All of the code here actually runs on the client side, it's magic! (gated by the "client" feature)

use crate::{program_args_builder::TopLevelProgramArgsBuilder, sh_capnp};

pub struct ShArgs<P: TopLevelProgramArgsBuilder> {
    pub program_args_builder: P,
    pub client: dusk_capnp::dusk_capnp::dusk::Client,
}

impl<P: TopLevelProgramArgsBuilder + Clone> dusk_capnp::dusk_capnp::program_args::Server
    for ShArgs<P>
{
    fn program_id(
        &mut self,
        _params: dusk_capnp::dusk_capnp::program_args::ProgramIdParams,
        mut results: dusk_capnp::dusk_capnp::program_args::ProgramIdResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        results.get().set_program_id(crate::sh_capnp::PROGRAM_ID);
        capnp::capability::Promise::ok(())
    }
}

impl<P: TopLevelProgramArgsBuilder + Clone> sh_capnp::sh_args::Server for ShArgs<P> {
    fn get(
        &mut self,
        _params: sh_capnp::sh_args::GetParams,
        mut results: sh_capnp::sh_args::GetResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        let builder = self.program_args_builder.clone();
        let client = capnp_rpc::new_client(builder);
        results.get().set_program_args_builder(client);
        results.get().set_client(self.client.clone());

        capnp::capability::Promise::ok(())
    }
}
