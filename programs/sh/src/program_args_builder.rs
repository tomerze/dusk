use std::vec::Vec;

use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::dusk_capnp::program_args;
use dusk_capnp::pry;
use linkme::distributed_slice;

use crate::sh_capnp;
use anyhow::Result;

#[derive(Copy, Clone)]
pub struct ProgramInfo {
    pub program_id: Option<u64>,
    pub name: &'static str,
    pub short_description: &'static str,
    pub long_description: &'static str,
    pub version: &'static str,
}

pub trait ProgramArgsBuilder {
    fn build(&self, client: dusk::Client, args: &str) -> Result<program_args::Client>;
}

pub trait TopLevelProgramArgsBuilder: sh_capnp::program_args_builder::Server + 'static {
    fn build_from_string(&self, s: &str) -> Result<program_args::Client>;
    fn get_available_programs_info(&self) -> Result<Vec<ProgramInfo>>;
}

impl<T: TopLevelProgramArgsBuilder> sh_capnp::program_args_builder::Server for T {
    fn build_from_string(
        &mut self,
        params: sh_capnp::program_args_builder::BuildFromStringParams,
        mut results: sh_capnp::program_args_builder::BuildFromStringResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        let s = pry!(pry!(pry!(params.get()).get_string()).to_str());
        let client = pry!(
            <Self as TopLevelProgramArgsBuilder>::build_from_string(self, s)
                .map_err(|e| { capnp::Error::failed(e.to_string()) })
        );

        results.get().set_result(client);
        Promise::ok(())
    }
}

pub struct StaticProgramArgsBuilderEntry {
    pub info: ProgramInfo,
    pub builder: Box<dyn ProgramArgsBuilder>,
}

#[distributed_slice]
pub static SH_PROGRAM_ARGS_BUILDERS: [fn() -> StaticProgramArgsBuilderEntry] = [..];

#[derive(Clone)]
pub struct StaticProgramArgsBuilder {
    client: dusk::Client,
}

impl StaticProgramArgsBuilder {
    pub fn new(client: dusk::Client) -> Self {
        Self { client }
    }
}

impl TopLevelProgramArgsBuilder for StaticProgramArgsBuilder {
    fn build_from_string(
        &self,
        s: &str,
    ) -> anyhow::Result<dusk_capnp::dusk_capnp::program_args::Client> {
        // split the string by the first space
        let (program_name, args) = s.split_once(' ').unwrap_or((s, ""));

        for entry in SH_PROGRAM_ARGS_BUILDERS {
            let entry = entry();
            if entry.info.name == program_name {
                log::debug!(
                    "building args from builder for program `{}` id {:?}",
                    program_name,
                    entry.info.program_id
                );
                return entry.builder.build(self.client.clone(), args);
            }
        }

        Err(anyhow::anyhow!(
            "no program args builder found for string `{}`",
            s
        ))
    }

    fn get_available_programs_info(&self) -> Result<Vec<ProgramInfo>> {
        let mut programs = vec![];
        for entry in SH_PROGRAM_ARGS_BUILDERS {
            let entry = entry();
            programs.push(entry.info);
        }
        Ok(programs)
    }
}
