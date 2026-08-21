use super::*;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, SH_ENTRIES, ShEntry};
use std::rc::Rc;

#[derive(clap::Parser)]
#[command(name = "programs", no_binary_name = true)]
struct ProgramsCli {}

struct ProgramsProgramArgsBuilder {}

#[dusk_program::async_trait::async_trait(?Send)]
impl ProgramArgsBuilder for ProgramsProgramArgsBuilder {
    async fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        ProgramsCli::try_parse_from(args)?;
        let program_names = SH_ENTRIES
            .iter()
            .map(|sh_entry| sh_entry())
            .filter_map(|sh_entry| {
                sh_entry
                    .info
                    .program_id
                    .map(|program_id| (program_id, sh_entry.info.name.to_string()))
            })
            .collect();
        Ok(Args::new(program_names).as_program_args()?)
    }
}

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(programs_capnp::PROGRAM_ID),
            name: "programs",
            short_description: "list available programs",
            long_description: r#"
The `programs` command lists the programs available on this node.
* Simpliy run `programs` to use it

The node identifies a program by its id and has no name for it, so the node
sends the list back to this client to be named and rendered. A program this
client has no shell entry for shows `N/A`, and a program with more than one
shell entry shows all of them.

Note:
The set of programs the node was compilied with might be different, or in different versions,
than the set of programs the client was compilied with. 

This program is used to see the programs the node was compiled with.
To see programs the client was compiled with, use the client builtin `help`.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(ProgramsProgramArgsBuilder {}),
    }
}
