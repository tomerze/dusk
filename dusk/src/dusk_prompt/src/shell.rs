use anyhow::Result;
use capnp::capability::FromClientHook;
use dusk_capnp::dusk_capnp::{dusk, process};
use dusk_program_sh::args::ShArgs;
use dusk_program_sh::program_args_builder::{
    ProgramInfo, StaticProgramArgsBuilder, TopLevelProgramArgsBuilder,
};
use dusk_program_sh::sh_capnp::{sh_args, sh_portal};
use std::hint::black_box;
use tracing::debug;

use crate::display_stream;

pub struct Shell {
    pub client: dusk::Client,
    pub sh_process: process::Client,
    pub hostname: String,
    pub available_programs_info: Vec<ProgramInfo>,
}

impl Shell {
    async fn get_sh_process_reconnect_callback(
        client: dusk::Client,
        static_program_args_builder: StaticProgramArgsBuilder,
    ) -> capnp::Result<process::Client> {
        let program_args =
            capnp_rpc::new_client::<sh_args::Client, ShArgs<StaticProgramArgsBuilder>>(ShArgs::<
                StaticProgramArgsBuilder,
            > {
                program_args_builder: static_program_args_builder,
                client: client.clone(),
            });
        let mut process_request = client.process_request();
        process_request.get().set_program_args(
            program_args.cast_to::<dusk_capnp::dusk_capnp::program_args::Client>(),
        );
        let process_reply = process_request.send().promise.await?;
        let process = process_reply.get()?.get_result()?;

        let mut run_request = client.run_request();
        run_request.get().set_process(process.clone());
        let _run_reply = run_request.send().promise.await?;
        Ok(process)
    }

    async fn get_sh_process(
        client: dusk::Client,
        static_program_args_builder: StaticProgramArgsBuilder,
    ) -> Result<process::Client> {
        let (process, _) = capnp_rpc::auto_reconnect(move || {
            Ok(capnp_rpc::new_future_client(
                Self::get_sh_process_reconnect_callback(
                    client.clone(),
                    static_program_args_builder.clone(),
                ),
            ))
        })?;

        let pid_reply = process.pid_request().send().promise.await?;
        let pid: u64 = pid_reply.get()?.get_result();

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
        let available_programs_info = static_program_args_builder
            .get_available_programs_info()?
            .into_iter()
            .collect::<Vec<_>>();
        let sh_process = Self::get_sh_process(client.clone(), static_program_args_builder).await?;

        Ok(Shell {
            client,
            sh_process,
            hostname: hostname.into(),
            available_programs_info,
        })
    }

    pub fn get_available_programs_info(&self) -> Vec<ProgramInfo> {
        self.available_programs_info.clone()
    }

    pub async fn process_command(&mut self, command: &str) -> Result<()> {
        let sh_process = self.sh_process.clone();

        let sh_portal = capnp_rpc::new_future_client(async move {
            let portal_request = sh_process.portal_request();
            let portal_reply = portal_request.send().promise.await?;
            Ok(portal_reply
                .get()?
                .get_result()?
                .cast_to::<sh_portal::Client>())
        });

        let mut sh_request = sh_portal.sh_request();
        sh_request.get().set_command(command);
        let (display_stream, done_receiver) = display_stream::DisplayStream::new_with_receiver();
        let display_stream: dusk_capnp::dusk_capnp::stream::Client =
            capnp_rpc::new_client(display_stream);
        sh_request.get().set_output(display_stream);
        let _sh_reply = sh_request.send().promise.await?;
        done_receiver.await?;
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
