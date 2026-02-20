extern crate linkme;

use super::*;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program_sh::entry::{ProgramArgsBuilder, ProgramInfo, ShEntry};
use linkme::distributed_slice;
use std::rc::Rc;

struct {{program-name | pascal_case}}ProgramArgsBuilder {}

impl ProgramArgsBuilder for {{program-name | pascal_case}}ProgramArgsBuilder {
    fn build(
        &self,
        client: dusk::Client,
        _args: &str,
    ) -> anyhow::Result<dusk_capnp::dusk_capnp::program_args::Client> {
        let client: {{program-name}}_capnp::{{program-name}}_args::Client =
            capnp_rpc::new_client(Args { client });
        Ok(client.cast_to::<dusk_capnp::dusk_capnp::program_args::Client>())
    }
}

#[distributed_slice(dusk_program_sh::entry::SH_ENTRIES)]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: ProgramInfo {
            program_id: Some({{program-name}}_capnp::PROGRAM_ID),
            name: "{{program-name}}",
            short_description: "TODO: short description",
            long_description: r#"
TODO: long description
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new({{program-name | pascal_case}}ProgramArgsBuilder {}),
    }
}
