use alloc::rc::Rc;

use crate::sh_capnp;
use capnp::capability::FromClientHook;
use dusk_capnp::capnp_rpc;
use dusk_capnp::dusk_capnp::{process, stream};
use dusk_program::anyhow;
use dusk_program::embassy_futures::select::{Either, select};
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::embassy_sync::signal::Signal;
use dusk_program::prelude::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program::stream::UndoneStream;

pub type Stop = Signal<CriticalSectionRawMutex, ()>;

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
        program_args: Rc<ProgramArgs>,
    ) -> Result<process::Client, ExecutionError> {
        let client = self.client.clone();
        let mut process_request = client.clone().process_request();
        program_args
            .with_reader(|reader| process_request.get().set_program_args(reader))
            .map_err(|e| ExecutionError::Runtime(e.into()))?;
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

    pub async fn program_args(
        &self,
        program_args: Rc<ProgramArgs>,
        stop: &Stop,
    ) -> Result<(), ExecutionError> {
        let process = self.execute_process(program_args).await?;

        let pid = process
            .pid_request()
            .send()
            .promise
            .await
            .map_err(|e| ExecutionError::Runtime(e.into()))?
            .get()
            .map_err(|e| ExecutionError::Runtime(e.into()))?
            .get_result();
        
        let (done, portal_error) = 'output: {
            let portal_reply = match process.portal_request().send().promise.await {
                Ok(reply) => reply,
                Err(err) => {
                    tracing::error!(pid = pid, error = err.to_string(), "failed to get process portal");
                    break 'output (true, None)
                },
            };
            let portal = match portal_reply.get().and_then(|r| r.get_result()) {
                Ok(portal) => portal.cast_to::<sh_capnp::output_portal::Client>(),
                Err(err) => {
                    tracing::error!(pid = pid, error = err.to_string(), "failed to get process portal reply");
                    break 'output (true, None)
                },
            };

            let (undone_stream, done_receiver) =
                UndoneStream::new_with_done_receiver(self.output.clone());
            let mut output_request = portal.output_request();
            output_request
                .get()
                .set_stream(capnp_rpc::new_client(undone_stream));

            match select(output_request.send().promise, stop.wait()).await {
                Either::First(result) => {
                    let done = done_receiver.await.is_ok();
                    let error = result.err().map(|e| {
                        ExecutionError::Program(
                            anyhow::Error::from(e).context("program output portal returned error"),
                        )
                    });
                    (done, error)
                }
                Either::Second(()) => {
                    stop.signal(());
                    tracing::info!(?pid, "stop signal sent to process");
                    (true, None)
                }
            }
        };

        // `done == false` is the wire-level signal for intentional
        // daemonization. Leave the process running and skip cleanup.
        let program_error = if done {
            let mut kill_request = self.client.kill_request();
            kill_request.get().set_pid(pid);
            kill_request.get().set_signal(15);
            let _ = kill_request.send().promise.await;

            let mut waitpid_request = self.client.waitpid_request();
            waitpid_request.get().set_pid(pid);
            let waitpid_error = waitpid_request.send().promise.await.err().map(|e| {
                ExecutionError::Program(anyhow::Error::from(e).context("program exited with error"))
            });

            // Portal error takes precedence over waitpid error.
            portal_error.or(waitpid_error)
        } else {
            portal_error
        };

        program_error.map_or(Ok(()), Err)
    }
}
