extern crate linkme;

use super::*;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program_sh::entry::{ProgramArgsBuilder, ProgramInfo, ShEntry};
use linkme::distributed_slice;
use std::rc::Rc;

struct PsProgramArgsBuilder {}

impl ProgramArgsBuilder for PsProgramArgsBuilder {
    fn build(
        &self,
        client: dusk::Client,
        _args: &str,
    ) -> anyhow::Result<dusk_capnp::dusk_capnp::program_args::Client> {
        let client: ps_capnp::ps_args::Client = capnp_rpc::new_client(Args { client });
        Ok(client.cast_to::<dusk_capnp::dusk_capnp::program_args::Client>())
    }
}

#[distributed_slice(dusk_program_sh::entry::SH_ENTRIES)]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: ProgramInfo {
            program_id: Some(ps_capnp::PROGRAM_ID),
            name: "ps",
            short_description: "list processes",
            long_description: r#"
The `ps` command is used to display information about the currently running processes.
* Use `ps` to list all currently running processes.
* Use `ps <pid>` to get more information about a specific process.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(PsProgramArgsBuilder {}),
    }
}
