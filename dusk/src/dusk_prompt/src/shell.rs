use anyhow::Result;
use capnp::capability::FromClientHook;
use dusk_capnp::dusk_capnp::{dusk, process};
use dusk_program_sh::args::ShArgs;
use dusk_program_sh::program_args_builder::{StaticProgramArgsBuilder, TopLevelProgramArgsBuilder};
use dusk_program_sh::sh_capnp::{sh_args, sh_portal};
use std::hint::black_box;
use tracing::debug;

pub struct Shell {
    pub client: dusk::Client,
    pub sh_process: process::Client,
    pub hostname: String,
    pub available_program_names: Vec<String>,
}

impl Shell {
    async fn get_sh_process(
        client: dusk::Client,
        static_program_args_builder: StaticProgramArgsBuilder,
    ) -> Result<process::Client> {
        let mut process_request = client.process_request();
        let program_args =
            capnp_rpc::new_client::<sh_args::Client, ShArgs<StaticProgramArgsBuilder>>(ShArgs::<
                StaticProgramArgsBuilder,
            > {
                program_args_builder: static_program_args_builder,
            });
        process_request.get().set_program_args(
            program_args.cast_to::<dusk_capnp::dusk_capnp::program_args::Client>(),
        );
        let process_reply = process_request.send().promise.await?;
        let process = process_reply.get()?.get_result()?;

        let pid_reply = process.pid_request().send().promise.await?;
        let pid: u64 = pid_reply.get()?.get_result();

        let mut run_request = client.run_request();
        run_request.get().set_process(process.clone());
        let _run_reply = run_request.send().promise.await?;

        debug!("sh started with pid {}", pid);
        Ok(process)
    }

    pub async fn new(client: dusk::Client) -> Result<Self> {
        let hostname_reply = client.hostname_request().send().promise.await?;
        let hostname = hostname_reply.get()?.get_result()?.to_str()?;

        // Unfortunately we need to trick the linker into including all
        // crates that register program args builders
        black_box(dusk_program_ps::program_args_builder_entry);

        let static_program_args_builder = StaticProgramArgsBuilder::default();
        let available_program_names = static_program_args_builder
            .get_available_program_names()?
            .into_iter()
            .map(String::from)
            .collect::<Vec<String>>();
        let sh_process = Self::get_sh_process(client.clone(), static_program_args_builder).await?;
        Ok(Shell {
            client,
            sh_process,
            hostname: hostname.into(),
            available_program_names,
        })
    }

    pub fn get_available_program_names(&self) -> Vec<String> {
        self.available_program_names.clone()
    }

    pub async fn process_command(&mut self, command: &str) -> Result<()> {
        let portal_request = self.sh_process.portal_request();
        let portal_reply = portal_request.send().promise.await?;

        let sh_portal = portal_reply
            .get()?
            .get_result()?
            .cast_to::<sh_portal::Client>();

        let mut sh_request = sh_portal.sh_request();
        sh_request.get().set_command(command);
        let _sh_reply = sh_request.send().promise.await?;

        Ok(())
    }

    pub async fn kill(self) -> Result<()> {
        let client = self.client.clone();
        let sh_process = self.sh_process.clone();
        let mut kill_request = client.kill_request();
        kill_request.get().set_process(sh_process.clone());
        kill_request.get().set_signal(15); // SIGTERM

        let _ = kill_request.send().promise.await?;

        Ok(())
    }
}
