use crate::entry::sh_entries;
use dusk_capnp::dusk_capnp::dusk;
use dusk_program::anyhow::{self, Context};
use dusk_program::program_args::ProgramArgs;
use std::rc::Rc;
use std::string::String;
use std::vec::Vec;

pub async fn program_args_for_command(
    client: dusk::Client,
    command: &str,
) -> anyhow::Result<Rc<ProgramArgs>> {
    let (remaining, words) = crate::parser::command_words(command)
        .map_err(|_| anyhow::anyhow!("invalid command `{}`", command))?;
    if !remaining.trim().is_empty() {
        anyhow::bail!("invalid command `{command}`");
    }
    let program = words
        .first()
        .ok_or_else(|| anyhow::anyhow!("empty command"))?;
    let builder = sh_entries()
        .iter()
        .find(|entry| entry.info.name == *program)
        .map(|entry| entry.program_args_builder.clone())
        .ok_or_else(|| anyhow::anyhow!("no sh entry found for `{program}`"))?;
    let arg_refs: Vec<&str> = words[1..].to_vec();
    builder
        .build(client, &arg_refs)
        .await
        .context("program args builder failed")
}
