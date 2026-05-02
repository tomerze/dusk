use crate::sh_capnp;
use capnp::capability::FromClientHook;
use dusk_capnp::capnp_rpc;
use dusk_capnp::dusk_capnp::program_args;
use dusk_capnp::dusk_capnp::{process, stream};
use dusk_program::anyhow;
use dusk_program::prelude::dusk;
use dusk_program::stream::UndoneStream;

pub enum ExecutionError {
    Runtime(anyhow::Error),
    Program(anyhow::Error),
}

pub struct Execution {
    client: dusk::Client,
    output: stream::Client,
}

impl Execution {
    pub fn new(client: dusk::Client, output: stream::Client) -> Self {
        Self { client, output }
    }

    async fn execute_process(
        &self,
        program_args: program_args::Client,
    ) -> Result<process::Client, ExecutionError> {
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

    async fn portal_process_and_pipe_output(
        &self,
        process: process::Client,
    ) -> Result<bool, ExecutionError> {
        // Portal acquisition — runtime errors
        let portal_reply = process
            .portal_request()
            .send()
            .promise
            .await
            .map_err(|e| ExecutionError::Runtime(e.into()))?;
        let portal = portal_reply
            .get()
            .map_err(|e| ExecutionError::Runtime(e.into()))?
            .get_result()
            .map_err(|e| ExecutionError::Runtime(e.into()))?
            .cast_to::<sh_capnp::output_portal::Client>();

        let (undone_stream, done_receiver) =
            UndoneStream::new_with_done_receiver(self.output.clone());
        let mut output_request = portal.output_request();
        output_request
            .get()
            .set_stream(capnp_rpc::new_client(undone_stream));

        // Script execution — program errors
        output_request
            .send()
            .promise
            .await
            .map_err(|e| ExecutionError::Program(e.into()))?;

        // Done signaling — sender drop is acceptable.
        Ok(done_receiver.await.is_ok())
    }

    pub async fn program_args(
        &self,
        program_args: program_args::Client,
    ) -> Result<(), ExecutionError> {
        let process = self.execute_process(program_args).await?;
        let done = self.portal_process_and_pipe_output(process.clone()).await?;

        // No done signal → the process daemonized itself by returning from
        // `output` without acking. Leave it running and report success.
        if !done {
            return Ok(());
        }

        let pid = process
            .pid_request()
            .send()
            .promise
            .await
            .map_err(|e| ExecutionError::Runtime(e.into()))?
            .get()
            .map_err(|e| ExecutionError::Runtime(e.into()))?
            .get_result();

        let mut kill_request = self.client.kill_request();
        kill_request.get().set_pid(pid);
        kill_request.get().set_signal(15);
        let _ = kill_request.send().promise.await;

        let mut waitpid_request = self.client.waitpid_request();
        waitpid_request.get().set_pid(pid);
        waitpid_request
            .send()
            .promise
            .await
            .map_err(|e| ExecutionError::Program(e.into()))?;

        Ok(())
    }
}
