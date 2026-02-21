use crate::anyhow::Context;
use crate::entry::ShEntriesBuilder;
use dusk_capnp::capnp::capability::Promise;
use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::pry;
use dusk_program::IntoCapnp;

use crate::sh_capnp::engine;

#[derive(Clone)]
pub struct ShEngine<S: ShEntriesBuilder> {
    client: dusk::Client,
    sh_entries_builder: S,
}

impl<S: ShEntriesBuilder> ShEngine<S> {
    pub fn new(client: dusk::Client, sh_entries_builder: S) -> Self {
        Self {
            client,
            sh_entries_builder,
        }
    }
}

impl<S: ShEntriesBuilder> engine::Server for ShEngine<S> {
    /// Takes a string like `ps 1234` and returns a ProgramArgs ready to run.
    fn build_program_args_from_string(
        &mut self,
        params: engine::BuildProgramArgsFromStringParams,
        mut results: engine::BuildProgramArgsFromStringResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        let s = pry!(pry!(pry!(params.get()).get_string()).to_str());
        // split the string by the first space
        let (program_name, args) = s.split_once(' ').unwrap_or((s, ""));
        let sh_entries = self.sh_entries_builder.get_entries();

        for entry in sh_entries {
            if entry.info.name == program_name {
                log::debug!(
                    "building args from program_args_builder for program `{}` id {:?}",
                    program_name,
                    entry.info.program_id
                );
                let client = pry!(
                    entry
                        .program_args_builder
                        .build(self.client.clone(), args)
                        .context("program args builder failed")
                        .into_capnp()
                );

                results.get().set_program_args(client);
                return Promise::ok(());
            }
        }
        Promise::err(capnp::Error::failed(alloc::format!(
            "no sh entry found for `{}`",
            program_name
        )))
    }

    fn client(
        &mut self,
        _params: engine::ClientParams,
        mut results: engine::ClientResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        results.get().set_client(self.client.clone());
        Promise::ok(())
    }
}
