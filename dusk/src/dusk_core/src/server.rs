use core::num::TryFromIntError;

use crate::driver;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;
use dusk_capnp::capnp;
use dusk_capnp::capnp::capability::Promise;
use dusk_capnp::capnp_rpc;
use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::dusk_capnp::process;
use dusk_capnp::pry;
use dusk_program::IntoCapnp;
use dusk_program::anyhow::Context;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::embassy_sync::signal::Signal;
use dusk_program::namespace::Namespace;
use dusk_program::process::Process;
use tracing::Instrument;
use tracing::debug;
use tracing::error;
use tracing::info_span;

pub struct DuskServer {
    namespace: Rc<Namespace>,
}

impl DuskServer {
    pub fn new(namespace: Rc<Namespace>) -> Self {
        DuskServer { namespace }
    }

    async fn run_inside_task(
        process_client: process::Client,
        namespace: Rc<Namespace>,
    ) -> Result<(), capnp::Error> {
        let ps_server_set = namespace.ps_server_set.lock().await;

        if let Some(process_server) = ps_server_set.get_local_server(&process_client).await {
            let process = process_server.borrow().server.clone_box();
            let task_id = Rc::new(Cell::new(0));
            let registered = Rc::new(Signal::<CriticalSectionRawMutex, ()>::new());
            let spawn_token = process_task(task_id.clone(), process, registered.clone())
                .map_err(|e| capnp::Error::failed(e.to_string()))?;
            task_id.set(spawn_token.id());
            namespace.spawner.spawn(spawn_token);
            // Wait for the process to be registered in the namespace maps before
            // replying — otherwise a follow-up `portal()` can race the registration
            // and see the pid as not-yet-running.
            registered.wait().await;
            Ok(())
        } else {
            Err(capnp::Error::failed("Process not found".to_string()))
        }
    }

    async fn new_process_client(
        process: Box<dyn Process>,
        namespace: Rc<Namespace>,
    ) -> Result<process::Client, capnp::Error> {
        let mut ps_server_set = namespace.ps_server_set.lock().await;
        // Creates a new client from the server and append the server to the set
        let client = ps_server_set.new_client(process);
        Ok(client)
    }
}

#[embassy_executor::task(pool_size = 16)]
async fn process_task(
    task_id: Rc<Cell<u32>>,
    process: Box<dyn Process>,
    registered: Rc<Signal<CriticalSectionRawMutex, ()>>,
) {
    let span = info_span!(
        "process",
        task_id = task_id.get(),
        pid = process.pid(),
        program_id = process.program_id(),
        program_name = process.name(),
        program_version = process.version(),
        namespace_id = process.namespace().id
    );
    dusk_program::embassy_futures::yield_now().await;
    if let Err(err) = process
        .bootstrap(Some(&registered))
        .instrument(span.clone())
        .await
    {
        // If bootstrap failed before signalling, unblock the run handler so it
        // doesn't hang forever waiting on a registration that never happens.
        registered.signal(());
        span.in_scope(|| {
            error!(
                pid = process.pid(),
                error = err.to_string(),
                "process bootstrap exited with error"
            )
        });
    }
}

impl dusk::Server for DuskServer {
    fn process(
        &mut self,
        params: dusk::ProcessParams,
        mut results: dusk::ProcessResults,
    ) -> Promise<(), capnp::Error> {
        debug!(method = "Dusk.process", "rpc call");
        let program_args_reader = pry!(pry!(params.get()).get_program_args());
        // Copy the incoming reader (and its capability table) into an owned
        // message so it outlives this RPC call.
        let program_args = pry!(dusk_program::program_args::ProgramArgs::from_reader(
            program_args_reader
        ));

        let namespace = self.namespace.clone();
        Promise::from_future(async move {
            let process = driver::process(namespace.clone(), program_args)
                .await
                .context("process creation failed")
                .into_capnp()?;

            let process_client = Self::new_process_client(process, namespace).await?;

            results.get().set_result(process_client);
            Ok(())
        })
    }

    fn run(
        &mut self,
        params: dusk::RunParams,
        mut _results: dusk::RunResults,
    ) -> Promise<(), capnp::Error> {
        debug!(method = "Dusk.run", "rpc call");
        let process = pry!(pry!(params.get()).get_process());

        Promise::from_future(Self::run_inside_task(process, self.namespace.clone()))
    }

    fn ps(
        &mut self,
        _params: dusk::PsParams,
        mut results: dusk::PsResults,
    ) -> Promise<(), capnp::Error> {
        debug!(method = "Dusk.ps", "rpc call");
        let namespace = self.namespace.clone();
        Promise::from_future(async move {
            let ps_vec: Vec<(u64, process::Client)> = {
                let ps_map_guard = namespace.ps_map.lock().await;
                ps_map_guard
                    .iter()
                    .map(|(k, v)| {
                        (
                            *k,
                            capnp_rpc::new_client::<process::Client, Box<dyn Process>>(
                                v.clone_box(),
                            ),
                        )
                    })
                    .collect()
            };
            let mut process_entries = results.get().init_process_entries(ps_vec.len() as u32);
            for (i, (pid, process)) in ps_vec.into_iter().enumerate() {
                let mut entry = process_entries.reborrow().get(
                    i.try_into()
                        .map_err(|e: TryFromIntError| capnp::Error::failed(e.to_string()))?,
                );
                entry.set_pid(pid);
                entry.set_process(process);
            }
            Ok(())
        })
    }

    fn kill(
        &mut self,
        params: dusk::KillParams,
        mut _results: dusk::KillResults,
    ) -> Promise<(), capnp::Error> {
        let pid = pry!(params.get()).get_pid();
        let signal = pry!(params.get()).get_signal();
        debug!(method = "Dusk.kill", ?pid, ?signal, "rpc call");

        let namespace = self.namespace.clone();
        Promise::from_future(async move {
            let channel = {
                let ps_signal_channel_map = namespace.ps_signal_channel_map.lock().await;
                ps_signal_channel_map.get(&pid).cloned()
            };

            match channel {
                Some(channel) => {
                    channel.sender().send(signal.into()).await;
                    Ok(())
                }
                None => Err(capnp::Error::failed(
                    "failed to find signal channel for process".to_string(),
                )),
            }
        })
    }

    fn waitpid(
        &mut self,
        params: dusk::WaitpidParams,
        mut _results: dusk::WaitpidResults,
    ) -> Promise<(), capnp::Error> {
        let pid = pry!(params.get()).get_pid();
        debug!(method = "Dusk.waitpid", ?pid, "rpc call");

        let namespace = self.namespace.clone();
        Promise::from_future(async move {
            let exit_watch = namespace.ps_exit_map.lock().await.get(&pid).cloned();
            let Some(exit_watch) = exit_watch else {
                return Err(capnp::Error::failed(
                    "failed to find exit watch for process".to_string(),
                ));
            };

            let mut receiver = exit_watch.receiver().ok_or_else(|| {
                capnp::Error::failed("failed to acquire receiver for process exit watch, maximum amount of receivers reached".into())
            })?;

            let mut changed = false;
            let result = loop {
                if let Some(result) = receiver.get().await {
                    break result;
                }
                if changed {
                    return Err(capnp::Error::failed(
                        "exit watch event fired but no result was found".to_string(),
                    ));
                }
                receiver.changed().await;
                changed = true;
            };

            namespace.ps_exit_map.lock().await.remove(&pid);

            result.map_err(capnp::Error::failed)
        })
    }

    fn hostname(
        &mut self,
        _params: dusk::HostnameParams,
        mut results: dusk::HostnameResults,
    ) -> Promise<(), capnp::Error> {
        debug!(method = "Dusk.hostname", "rpc call");

        let hostname = match driver::hostname() {
            Ok(id) => id,
            Err(err) => {
                error!("failed to receive hostname from driver: `{err:#?}`");
                String::from("error")
            }
        };
        results.get().set_result(&hostname[..]);
        Promise::ok(())
    }
}
