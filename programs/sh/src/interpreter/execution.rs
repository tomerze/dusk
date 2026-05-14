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

        // Race output completion against external cancellation. Either path
        // produces a (done, program_error) pair so the cleanup logic below
        // runs regardless of how `output` finished.
        let (done, program_error) = match select(
            output_request.send().promise,
            stop.wait(),
        )
        .await
        {
            Either::First(result) => {
                // `output` returned. Drain `done_receiver` unconditionally —
                // `UndoneStream`'s sender is dropped when `output` ends, so
                // this resolves either to `Ok(())` (program called `done`)
                // or `Err(_)` (program declared itself a daemon). `done` is
                // orthogonal to whether `output` returned `Ok` or `Err`.
                let done = done_receiver.await.is_ok();
                let error = result.err().map(|e| ExecutionError::Program(e.into()));
                (done, error)
            }
            Either::Second(()) => {
                // Re-signal so callers up the stack also observe the stop.
                stop.signal(());
                (true, None)
            }
        };

        // `done == false` is the wire-level signal for intentional
        // daemonization. Leave the process running and skip cleanup.
        if done {
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
        }

        // Propagate the program error after cleanup so failed processes that
        // ack'd `done` don't leak.
        if let Some(e) = program_error {
            return Err(e);
        }
        Ok(())
    }
}
