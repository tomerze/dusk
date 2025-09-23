// All of the code here actually runs on the client side, it's magic! (gated by the "client" feature)
use capnp::capability::Promise;

use crate::sh_capnp;

pub trait ProgramArgsBuilder: Clone + sh_capnp::program_args_builder::Server + 'static {
    fn build_from_string(&self, s: &str) -> dusk_capnp::dusk_capnp::program_args::Client;
}

/// While a normal `ProgramArgsBuilder` expects to build args for a specific program,
/// a `TopLevelProgramArgsBuilder` expects the program ID to be communicated as well.
///
/// Example:
/// ```rust
/// kill_program_args_builder.build_from_string("-9 1243") // A `ProgramArgsBuilder`
/// ```
/// vs
/// ```rust
/// top_level_program_args_builder.build_from_string("kill -9 1243") // A `TopLevelProgramArgsBuilder`
/// ```
pub trait TopLevelProgramArgsBuilder: ProgramArgsBuilder {}

impl<T: ProgramArgsBuilder> sh_capnp::program_args_builder::Server for T {
    fn build_from_string(
        &mut self,
        _: sh_capnp::program_args_builder::BuildFromStringParams,
        _: sh_capnp::program_args_builder::BuildFromStringResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        // TODO: fix this
        //let s = pry!(pry!(params.get()).get_string());
        <Self as ProgramArgsBuilder>::build_from_string(self, "");
        Promise::ok(())
    }
}

#[derive(Clone, Default)]
pub struct StaticProgramArgsBuilder {}

impl ProgramArgsBuilder for StaticProgramArgsBuilder {
    fn build_from_string(&self, _s: &str) -> dusk_capnp::dusk_capnp::program_args::Client {
        // TODO: fix this
        capnp_rpc::new_client(ShArgs::<StaticProgramArgsBuilder> {
            program_args_builder: StaticProgramArgsBuilder::default(),
        })
    }
}

impl TopLevelProgramArgsBuilder for StaticProgramArgsBuilder {}

pub struct ShArgs<P: TopLevelProgramArgsBuilder> {
    pub program_args_builder: P,
}

impl Default for ShArgs<StaticProgramArgsBuilder> {
    fn default() -> Self {
        ShArgs::<StaticProgramArgsBuilder> {
            program_args_builder: StaticProgramArgsBuilder::default(),
        }
    }
}

impl<P: TopLevelProgramArgsBuilder> dusk_capnp::dusk_capnp::program_args::Server for ShArgs<P> {
    fn program_id(
        &mut self,
        _params: dusk_capnp::dusk_capnp::program_args::ProgramIdParams,
        mut results: dusk_capnp::dusk_capnp::program_args::ProgramIdResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        results.get().set_program_id(crate::sh_capnp::PROGRAM_ID);
        capnp::capability::Promise::ok(())
    }
}

impl<P: TopLevelProgramArgsBuilder> sh_capnp::sh_args::Server for ShArgs<P> {
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
