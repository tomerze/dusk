use alloc::{boxed::Box, format, rc::Rc, string::ToString};
use anyhow::{Context, Result};
use dusk_capnp::capnp;
use dusk_capnp::capnp::capability::Promise;
use dusk_capnp::dusk_capnp::portal;
use dusk_capnp::dusk_capnp::process;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::DynamicReceiver;
use embassy_sync::signal::Signal;
use tracing::Instrument;

use crate::IntoCapnp;
use crate::namespace::Namespace;
use crate::program_args::ProgramArgs;
use crate::ready::Ready;
use crate::signal;

#[derive(Clone)]
pub struct ProcessContext {
    pub pid: u64,
    pub namespace: alloc::rc::Rc<Namespace>,
    pub program_args: Rc<ProgramArgs>,
    pub name: Rc<
        embassy_sync::blocking_mutex::Mutex<
            embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
            core::cell::RefCell<Option<alloc::string::String>>,
        >,
    >,
}

impl core::fmt::Debug for ProcessContext {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let name = self.name.lock(|name| name.borrow().clone());
        formatter
            .debug_struct("ProcessContext")
            .field("pid", &self.pid)
            .field("name", &name)
            .field("program_args", &self.program_args)
            .finish_non_exhaustive()
    }
}

// You are probably wondering how processes are run.
// There are two ways, inside a task of their own or inside the session task.
// A process running inside a task of its own doesn't depend on the session it was created from, essentially making it a daemon.
// Here are some flow charts for you:
// 1. Run in task flow: DuskServer.run -> DuskServer.run_inside_task -> process_task -> Process.bootstrap -> Process.main -> ProcessMixin.main
// 2. Run in session flow: Process.run -> Process.bootstrap -> Process.main -> ProcessMixin.main
//
// And how does one get a process?
// Dusk.process -> Driver.process ->
// LauncherSet.launch (reads program_id from the args message) ->
// Launcher.launch -> Process
// Cool right?
//
// Note: Never run process.main directly, always use process.bootstrap.
#[async_trait::async_trait(?Send)]
pub trait Process: ProcessMixin {
    fn program_id(&self) -> u64;
    fn name(&self) -> alloc::string::String;
    fn version(&self) -> alloc::string::String;
    fn clone_box(&self) -> Box<dyn Process>;
    fn namespace(&self) -> Rc<Namespace>;
    fn pid(&self) -> u64;
}

#[async_trait::async_trait(?Send)]
pub trait ProcessMixin {
    fn portal(&self) -> portal::Client;
    async fn main(
        &self,
        signal_receiver: DynamicReceiver<'async_trait, signal::Signal>,
        ready: Ready,
    ) -> Result<()>;
}

impl dyn Process {
    pub async fn bootstrap(
        &self,
        registered: Option<&Signal<CriticalSectionRawMutex, ()>>,
    ) -> Result<()> {
        let namespace = self.namespace();
        let pid = self.pid();
        let channel = namespace
            .ps_signal_channel_map
            .lock()
            .await
            .get(&pid)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!("pid {pid} does not exist in namespace {}", namespace.id)
            })?;
        let signal_receiver = channel.dyn_receiver();
        let ready = namespace
            .ps_ready_map
            .lock()
            .await
            .get(&pid)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("pid {pid} has no ready watch"))?;
        let suspended = namespace
            .ps_suspended_map
            .lock()
            .await
            .get(&pid)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("pid {pid} has no suspended watch"))?;
        if let Some(registered) = registered {
            registered.signal(());
        }
        if !suspended.try_get().unwrap_or(false) {
            tracing::info!(pid, "process is already running");
            return Ok(());
        }
        suspended.sender().send(false);

        tracing::info!("main run");
        let result = self
            .main(signal_receiver, ready)
            .instrument(tracing::Span::current())
            .await;
        let error = result.as_ref().err().map(|e| e.to_string());
        tracing::info!(error = error, "main exit");

        namespace
            .unregister(pid, result.map_err(|e| e.to_string()))
            .await;
        Ok(())
    }
}

impl process::Server for dyn Process {
    fn pid(
        &mut self,
        _params: process::PidParams,
        mut results: process::PidResults,
    ) -> Promise<(), capnp::Error> {
        results.get().set_result(Process::pid(self));

        Promise::ok(())
    }

    fn program_id(
        &mut self,
        _params: process::ProgramIdParams,
        mut results: process::ProgramIdResults,
    ) -> Promise<(), capnp::Error> {
        results.get().set_result(Process::program_id(self));

        Promise::ok(())
    }

    fn name(
        &mut self,
        _params: process::NameParams,
        mut results: process::NameResults,
    ) -> Promise<(), capnp::Error> {
        results.get().set_result(Process::name(self));

        Promise::ok(())
    }

    fn version(
        &mut self,
        _params: process::VersionParams,
        mut results: process::VersionResults,
    ) -> Promise<(), capnp::Error> {
        results.get().set_result(Process::version(self));

        Promise::ok(())
    }

    fn portal(
        &mut self,
        _params: process::PortalParams,
        mut results: process::PortalResults,
    ) -> Promise<(), capnp::Error> {
        let namespace = self.namespace();
        let pid = <Self as Process>::pid(self);

        let process = self.clone_box();
        Promise::from_future(async move {
            let process_exists = {
                let ps_map = namespace.ps_map.lock().await;
                ps_map.contains_key(&pid)
            };
            if !process_exists {
                return Err(capnp::Error::failed(
                    "process no longer exists, cannot get portal".to_string(),
                ));
            }
            let suspended = {
                let ps_suspended_map = namespace.ps_suspended_map.lock().await;
                ps_suspended_map.get(&pid).cloned()
            };
            if let Some(suspended) = suspended {
                let mut suspended_receiver = suspended.receiver().ok_or_else(|| {
                    capnp::Error::failed(
                        "couldn't acquire receiver for process suspended watch".into(),
                    )
                })?;
                while suspended_receiver.get().await {
                    suspended_receiver.changed().await;
                }
            }
            let ready = {
                let ps_ready_map = namespace.ps_ready_map.lock().await;
                ps_ready_map.get(&pid).cloned()
            };
            let exit = {
                let ps_exit_map = namespace.ps_exit_map.lock().await;
                ps_exit_map.get(&pid).cloned()
            };
            let (ready, exit) = match (ready, exit) {
                (Some(ready), Some(exit)) => (ready, exit),
                (None, Some(_)) => {
                    return Err(capnp::Error::failed(
                        "process does not appear in the ready map, but does appear in exit map, likely mid-teardown".to_string(),
                    ));
                }
                (Some(_), None) => {
                    return Err(capnp::Error::failed(
                        "process does not appear in the exit map, but does appear in ready map, this is a faulty state".to_string(),
                    ));
                }
                (None, None) => {
                    return Err(capnp::Error::failed(
                        "process likely no longer exists".to_string(),
                    ));
                }
            };

            let mut ready_receiver = ready.receiver().ok_or_else(|| {
                capnp::Error::failed("couldn't acquire receiver for process ready watch".into())
            })?;
            let mut exit_receiver = exit.receiver().ok_or_else(|| {
                capnp::Error::failed("couldn't acquire receiver for process exit watch".into())
            })?;

            let became_ready = async {
                while !ready_receiver.get().await {
                    ready_receiver.changed().await;
                }
            };
            let exited = async {
                loop {
                    if let Some(result) = exit_receiver.get().await {
                        return result;
                    }
                    exit_receiver.changed().await;
                }
            };

            let outcome = select(exited, became_ready).await;
            let exit_result = match outcome {
                Either::First(exit_result) if !ready_receiver.get().await => Some(exit_result),
                _ => None,
            };
            match exit_result {
                Some(Err(error)) => Err(capnp::Error::failed(format!(
                    "while waiting for process to become ready it exited with error: {error}"
                ))),
                Some(Ok(())) => Err(capnp::Error::failed(
                    "while waiting for process to become ready it exited with no error".to_string(),
                )),
                None => {
                    let portal = <Self as ProcessMixin>::portal(&*process);
                    results.get().set_result(portal);
                    Ok(())
                }
            }
        })
    }

    fn run(
        &mut self,
        _params: process::RunParams,
        mut _results: process::RunResults,
    ) -> Promise<(), capnp::Error> {
        let process = self.clone_box();
        let span = tracing::Span::current();

        span.record("pid", Self::pid(self));
        span.record("program_id", Self::program_id(self));
        span.record("program_name", Self::name(self));
        span.record("program_version", Self::version(self));
        Promise::from_future(async move {
            process
                .bootstrap(None)
                .instrument(tracing::Span::current())
                .await
                .context("process bootstrap failed")
                .into_capnp()
        })
    }
}

impl process::Server for Box<dyn Process> {
    fn pid(
        &mut self,
        params: process::PidParams,
        results: process::PidResults,
    ) -> Promise<(), capnp::Error> {
        <dyn Process as process::Server>::pid(&mut **self, params, results)
    }

    fn program_id(
        &mut self,
        params: process::ProgramIdParams,
        results: process::ProgramIdResults,
    ) -> Promise<(), capnp::Error> {
        <dyn Process as process::Server>::program_id(&mut **self, params, results)
    }

    fn name(
        &mut self,
        params: process::NameParams,
        results: process::NameResults,
    ) -> Promise<(), capnp::Error> {
        <dyn Process as process::Server>::name(&mut **self, params, results)
    }

    fn version(
        &mut self,
        params: process::VersionParams,
        results: process::VersionResults,
    ) -> Promise<(), capnp::Error> {
        <dyn Process as process::Server>::version(&mut **self, params, results)
    }

    fn portal(
        &mut self,
        params: process::PortalParams,
        results: process::PortalResults,
    ) -> Promise<(), capnp::Error> {
        <dyn Process as process::Server>::portal(&mut **self, params, results)
    }

    fn run(
        &mut self,
        params: process::RunParams,
        results: process::RunResults,
    ) -> Promise<(), capnp::Error> {
        <dyn Process as process::Server>::run(&mut **self, params, results)
    }
}
