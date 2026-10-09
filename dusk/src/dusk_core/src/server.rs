use core::num::TryFromIntError;

use crate::driver;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;
use dusk_capnp::capnp;
use dusk_capnp::capnp::capability::Promise;
use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::dusk_capnp::process;
use dusk_capnp::pry;
use dusk_program::IntoCapnp;
use dusk_program::anyhow::Context;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::embassy_sync::signal::Signal;
use dusk_program::namespace::Namespace;
use dusk_program::namespace::PsEntry;
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
        let process = {
            let ps_server_set = namespace.ps_server_set.lock().await;
            ps_server_set
                .get_local_server(&process_client)
                .await
                .map(|process_server| process_server.borrow().server.clone_box())
        };

        if let Some(process) = process {
            let task_id = Rc::new(Cell::new(0));
            let registered = Rc::new(Signal::<CriticalSectionRawMutex, ()>::new());
            let spawn_token = process_task(task_id.clone(), process, registered.clone())
                .map_err(|e| capnp::Error::failed(e.to_string()))?;
            task_id.set(spawn_token.id());
            namespace.spawner.spawn(spawn_token);
            // Wait for the process to be registered in the namespace maps before
            // replying - otherwise a follow-up `portal()` can race the registration
            // and see the pid as not-yet-running.
            registered.wait().await;
            Ok(())
        } else {
            Err(capnp::Error::failed("process not found".to_string()))
        }
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
        __new_task_id__ = task_id.get(),
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
            let launcher_set = crate::launchers::launchers(namespace.id)
                .context("launcher set lookup failed")
                .into_capnp()?;
            let process = namespace
                .clone()
                .process(launcher_set, program_args)
                .await
                .context("process creation failed")
                .into_capnp()?;

            results.get().set_result(process);
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
            let ps_vec: Vec<(u64, PsEntry)> = {
                let ps_map = namespace.ps_map.lock().await;
                ps_map
                    .iter()
                    .map(|(pid, entry)| (*pid, entry.clone()))
                    .collect()
            };
            let mut process_entries = results.get().init_process_entries(ps_vec.len() as u32);
            for (i, (pid, entry)) in ps_vec.into_iter().enumerate() {
                let mut process_entry = process_entries.reborrow().get(
                    i.try_into()
                        .map_err(|e: TryFromIntError| capnp::Error::failed(e.to_string()))?,
                );
                process_entry.set_pid(pid);
                process_entry.set_process(entry.process);
                process_entry.set_ready(entry.ready.try_get().unwrap_or(false));
                process_entry.set_suspended(entry.suspended.try_get().unwrap_or(false));
                process_entry.set_exited(entry.exit.try_get().flatten().is_some());
            }
            Ok(())
        })
    }

    fn programs(
        &mut self,
        _params: dusk::ProgramsParams,
        mut results: dusk::ProgramsResults,
    ) -> Promise<(), capnp::Error> {
        debug!(method = "Dusk.programs", "rpc call");
        let namespace = self.namespace.clone();
        Promise::from_future(async move {
            let launcher_set = crate::launchers::launchers(namespace.id)
                .context("couldn't get the namespace's launcher set")
                .into_capnp()?;
            let launchers = launcher_set.launchers.lock().await;
            let mut program_entries = results.get().init_program_entries(launchers.len() as u32);
            for (i, launcher) in launchers.iter().enumerate() {
                let mut entry = program_entries.reborrow().get(
                    i.try_into()
                        .map_err(|e: TryFromIntError| capnp::Error::failed(e.to_string()))?,
                );
                let program_id = launcher.program_id();
                let version = launcher.version();
                let git_revision = launcher.git_rev();
                entry.set_program_id(program_id);
                entry.set_version(&version[..]);
                entry.set_git_revision(&git_revision[..]);
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
        debug!(method = "Dusk.kill", pid, signal, "rpc call");

        let namespace = self.namespace.clone();
        Promise::from_future(async move { namespace.kill(pid, signal.into()).await.into_capnp() })
    }

    fn waitpid(
        &mut self,
        params: dusk::WaitpidParams,
        mut _results: dusk::WaitpidResults,
    ) -> Promise<(), capnp::Error> {
        let pid = pry!(params.get()).get_pid();
        debug!(method = "Dusk.waitpid", pid, "rpc call");

        let namespace = self.namespace.clone();
        Promise::from_future(async move {
            let Some(entry) = namespace.entry(pid).await else {
                return Err(capnp::Error::failed("couldn't find process".to_string()));
            };

            let mut receiver = entry.exit.receiver().ok_or_else(|| {
                capnp::Error::failed("couldn't acquire receiver for process exit watch, maximum amount of receivers reached".into())
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

            namespace.unregister(pid, &entry.exit).await;

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
                error!("didn't receive hostname from driver: `{err:#?}`");
                String::from("error")
            }
        };
        results.get().set_result(&hostname[..]);
        Promise::ok(())
    }

    fn time(
        &mut self,
        _params: dusk::TimeParams,
        mut results: dusk::TimeResults,
    ) -> Promise<(), capnp::Error> {
        debug!(method = "Dusk.time", "rpc call");
        let time = self
            .namespace
            .creation_time
            .load(core::sync::atomic::Ordering::Relaxed)
            + dusk_program::embassy_time::Instant::now().as_millis();
        results.get().set_unix_time_ms(time);
        Promise::ok(())
    }

    fn id(
        &mut self,
        _params: dusk::IdParams,
        mut results: dusk::IdResults,
    ) -> Promise<(), capnp::Error> {
        debug!(method = "Dusk.id", "rpc call");
        results.get().set_result(self.namespace.id);
        Promise::ok(())
    }

    fn settime(
        &mut self,
        params: dusk::SettimeParams,
        _results: dusk::SettimeResults,
    ) -> Promise<(), capnp::Error> {
        let unix_time_ms = pry!(params.get()).get_unix_time_ms();
        debug!(method = "Dusk.settime", unix_time_ms, "rpc call");
        self.namespace.creation_time.store(
            unix_time_ms - dusk_program::embassy_time::Instant::now().as_millis(),
            core::sync::atomic::Ordering::Relaxed,
        );
        Promise::ok(())
    }
}
