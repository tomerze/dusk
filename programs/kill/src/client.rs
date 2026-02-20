extern crate linkme;

use super::*;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program_sh::entry::{ProgramArgsBuilder, ProgramInfo, ShEntry};
use linkme::distributed_slice;
use std::rc::Rc;

struct KillProgramArgsBuilder {}

impl ProgramArgsBuilder for KillProgramArgsBuilder {
    fn build(
        &self,
        client: dusk::Client,
        _args: &str,
    ) -> anyhow::Result<dusk_capnp::dusk_capnp::program_args::Client> {
        let client: kill_capnp::kill_args::Client = capnp_rpc::new_client(Args { client });
        Ok(client.cast_to::<dusk_capnp::dusk_capnp::program_args::Client>())
    }
}

#[distributed_slice(dusk_program_sh::entry::SH_ENTRIES)]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: ProgramInfo {
            program_id: Some(kill_capnp::PROGRAM_ID),
            name: "kill",
            short_description: "TODO: short description",
            long_description: r#"
TODO: long description
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(KillProgramArgsBuilder {}),
    }
}
