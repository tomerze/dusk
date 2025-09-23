use capnp::capability::Promise;

use crate::sh_capnp;

// All of the code here actually runs on the client side, it's magic!
pub trait ProgramArgsBuilder: Clone + sh_capnp::program_args_builder::Server + 'static {
    fn build_from_string(&self, s: &str) -> dusk_capnp::dusk_capnp::program_args::Client;
}

impl<T: ProgramArgsBuilder> sh_capnp::program_args_builder::Server for T {
    fn build_from_string(
        &mut self,
        _: sh_capnp::program_args_builder::BuildFromStringParams,
        _: sh_capnp::program_args_builder::BuildFromStringResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        //let s = pry!(pry!(params.get()).get_string());
        <Self as ProgramArgsBuilder>::build_from_string(self, "");
        Promise::ok(())
    }
}

#[derive(Clone, Default)]
pub struct StaticProgramArgsBuilder {}

impl ProgramArgsBuilder for StaticProgramArgsBuilder {
    fn build_from_string(&self, _s: &str) -> dusk_capnp::dusk_capnp::program_args::Client {
        // todo fix this
        capnp_rpc::new_client(ShArgs::<StaticProgramArgsBuilder> {
            program_args_builder: StaticProgramArgsBuilder::default(),
        })
    }
}

pub struct ShArgs<P: ProgramArgsBuilder> {
    pub program_args_builder: P,
}

impl Default for ShArgs<StaticProgramArgsBuilder> {
    fn default() -> Self {
        ShArgs::<StaticProgramArgsBuilder> {
            program_args_builder: StaticProgramArgsBuilder::default(),
        }
    }
}

impl<P: ProgramArgsBuilder> dusk_capnp::dusk_capnp::program_args::Server for ShArgs<P> {
    fn program_id(
        &mut self,
        _params: dusk_capnp::dusk_capnp::program_args::ProgramIdParams,
        mut results: dusk_capnp::dusk_capnp::program_args::ProgramIdResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        results.get().set_program_id(crate::sh_capnp::PROGRAM_ID);
        capnp::capability::Promise::ok(())
    }
}

impl<P: ProgramArgsBuilder> sh_capnp::sh_args::Server for ShArgs<P> {
    fn program_args_builder(
        &mut self,
        _params: sh_capnp::sh_args::ProgramArgsBuilderParams,
        mut results: sh_capnp::sh_args::ProgramArgsBuilderResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        let builder = self.program_args_builder.clone();
        let client = capnp_rpc::new_client(builder);
        results.get().set_result(client);
        capnp::capability::Promise::ok(())
    }
}
