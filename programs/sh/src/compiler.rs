use crate::anyhow::{Context, Result};
use crate::entry::ShEntriesBuilder;

use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::dusk_capnp::program_args;
use dusk_program::IntoCapnp;
use dusk_program::anyhow::anyhow;

#[derive(Clone)]
pub struct Compiler<S: ShEntriesBuilder> {
    client: dusk::Client,
    sh_entries_builder: S,
}

impl<S: ShEntriesBuilder> Compiler<S> {
    pub fn new(client: dusk::Client, sh_entries_builder: S) -> Self {
        Self {
            client,
            sh_entries_builder,
        }
    }

    /// Takes a string like `kill 1234` and returns a ProgramArgs ready to run.
    fn compile_program_args_from_string(&mut self, s: &str) -> Result<program_args::Client> {
        // split the string by the first space
        let (program_name, args) = s.split_once(' ').unwrap_or((s, ""));
        let sh_entries = self.sh_entries_builder.get_entries();

        for entry in sh_entries {
            if entry.info.name == program_name {
                let client = entry
                    .program_args_builder
                    .build(self.client.clone(), args)
                    .context("program args builder failed")
                    .into_capnp()?;

                return Ok(client);
            }
        }
        Err(anyhow!("no sh entry found for `{}`", program_name))
    }

    pub fn compile(
        &mut self,
        s: &str,
        mut builder: crate::sh_capnp::script::Builder,
    ) -> Result<()> {
        let (program_args_string, background) = match s.split_once('&') {
            Some((program_args, should_be_empty)) => {
                if !should_be_empty.is_empty() {
                    return Err(anyhow!(
                        "syntax error, found `{}` after `&`",
                        should_be_empty
                    ));
                }
                (program_args, true)
            }
            None => (s, false),
        };

        builder.set_program_args(self.compile_program_args_from_string(program_args_string)?);
        builder.set_background(background);

        Ok(())
    }
}
