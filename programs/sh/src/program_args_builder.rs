use capnp::capability::Promise;
use dusk_capnp::pry;
use linkme::distributed_slice;

use crate::sh_capnp;
use anyhow::Result;

pub trait ProgramArgsBuilder: sh_capnp::program_args_builder::Server + 'static {
    fn build_from_string(&self, s: &str) -> Result<dusk_capnp::dusk_capnp::program_args::Client>;
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
        params: sh_capnp::program_args_builder::BuildFromStringParams,
        mut results: sh_capnp::program_args_builder::BuildFromStringResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        let s = pry!(pry!(pry!(params.get()).get_string()).to_str());
        let client = pry!(<Self as ProgramArgsBuilder>::build_from_string(self, s)
            .map_err(|e| { capnp::Error::failed(e.to_string()) }));

        results.get().set_result(client);
        Promise::ok(())
    }
}

pub struct StaticProgramArgsBuilderEntry {
    program_id: u64,
    name: &'static str,
    builder: Box<dyn ProgramArgsBuilder>,
}

#[distributed_slice]
pub static SH_PROGRAM_ARGS_BUILDERS: [fn() -> StaticProgramArgsBuilderEntry] = [..];

#[derive(Clone, Default)]
pub struct StaticProgramArgsBuilder {}

impl ProgramArgsBuilder for StaticProgramArgsBuilder {
    fn build_from_string(
        &self,
        s: &str,
    ) -> anyhow::Result<dusk_capnp::dusk_capnp::program_args::Client> {
        // split the string by the first space
        let (program_name, args) = s.split_once(' ').unwrap_or((s, ""));

        for entry in SH_PROGRAM_ARGS_BUILDERS {
            let entry = entry();
            if entry.name == program_name {
                log::debug!(
                    "building args from builder for program `{}` id {}",
                    program_name,
                    entry.program_id
                );
                return entry.builder.build_from_string(args);
            }
        }

        Err(anyhow::anyhow!(
            "no program args builder found for string `{}`",
            s
        ))
    }
}

impl crate::program_args_builder::TopLevelProgramArgsBuilder for StaticProgramArgsBuilder {}
