use super::*;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
use std::rc::Rc;

#[derive(clap::Parser)]
#[command(name = "kvs", no_binary_name = true)]
struct KvsCli {}

struct KvsProgramArgsBuilder {}

#[dusk_program::async_trait::async_trait(?Send)]
impl ProgramArgsBuilder for KvsProgramArgsBuilder {
    async fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let _cli = KvsCli::try_parse_from(args)?;
        Ok(Args::new().as_program_args()?)
    }
}

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(kvs_capnp::PROGRAM_ID),
            name: "kvs",
            short_description: "key-value store (empty skeleton, does nothing yet)",
            long_description: r#"
The `kvs` command is an empty skeleton for a key-value store. It currently
does nothing and exits successfully.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(KvsProgramArgsBuilder {}),
    }
}
