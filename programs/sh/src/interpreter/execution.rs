use crate::sh_capnp;
use capnp::capability::FromClientHook;
use dusk_capnp::capnp_rpc;
use dusk_capnp::dusk_capnp::program_args;
use dusk_capnp::dusk_capnp::{process, stream};
use dusk_program::anyhow::{self, Result, anyhow};
use dusk_program::prelude::dusk;
use dusk_program::stream::UndoneStream;

#[derive(Clone)]
pub enum Mode {
    Background(),
    Output(stream::Client),
}

pub struct Execution {
    client: dusk::Client,
    mode: Mode,
}

impl Execution {
    pub fn new(client: dusk::Client, mode: Mode) -> Self {
        Self { client, mode }
    }

    async fn execute_process(
        &self,
        program_args: dusk_capnp::dusk_capnp::program_args::Client,
    ) -> anyhow::Result<process::Client> {
        let client = self.client.clone();
        let mut process_request = client.clone().process_request();
        process_request.get().set_program_args(program_args);
        let process = capnp_rpc::new_future_client(async move {
            let process_reply = process_request.send().promise.await?;
            let process = process_reply.get()?.get_result()?;
            let mut run_request = client.run_request();
            run_request.get().set_process(process.clone());
            let _run_reply = run_request.send().promise.await?;
            Ok(process)
        });

        Ok(process)
    }

    async fn portal_process_and_pipe_output(&self, process: process::Client) -> anyhow::Result<()> {
        let output = match &self.mode {
            Mode::Background() => {
                return Err(anyhow!(
                    "this functions should never be called for background processes"
                ));
            }
            Mode::Output(output) => output,
        };
        let portal: sh_capnp::output_portal::Client = capnp_rpc::new_future_client(async move {
            let portal_request = process.portal_request();
            let portal_reply = portal_request.send().promise.await?;
            Ok(portal_reply
                .get()?
                .get_result()?
                .cast_to::<sh_capnp::output_portal::Client>())
        });

        let (undone_stream, done_receiver) = UndoneStream::new_with_done_receiver(output.clone());

        let mut output_request = portal.output_request();
        output_request
            .get()
            .set_stream(capnp_rpc::new_client(undone_stream));
        let _output_reply = output_request.send().promise.await?;
        done_receiver.await.map_err(|e| anyhow::anyhow!("{}", e))?;

        Ok(())
    }

    pub async fn program_args(&self, program_args: program_args::Client) -> Result<()> {
        let process = self.execute_process(program_args).await?;

        if let Mode::Background() = self.mode {
            // Since `process` is a future client we need to somehow trigger it's creation.
            let _pid = process.pid_request().send().promise.await?;
            return Ok(());
        };

        self.portal_process_and_pipe_output(process.clone()).await?;
        let pid = process
            .pid_request()
            .send()
            .promise
            .await?
            .get()?
            .get_result();

        let mut kill_request = self.client.kill_request();
        kill_request.get().set_pid(pid);
        kill_request.get().set_signal(15); // SIGTERM
        kill_request.send().promise.await?;

        Ok(())
    }
}
