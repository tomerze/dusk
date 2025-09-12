use crate::signal::Signal;
use alloc::{boxed::Box, rc::Rc, string::ToString};
use anyhow::Result;
use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::portal;
use dusk_capnp::dusk_capnp::process;
use log::debug;

use crate::namespace::Namespace;

// You are probably wondering how processes are run.
// There are two ways, inside a task of their own or inside the session task.
// A process running inside a task of its own doesn't depend on the session it was created from, essentially making it a daemon.
// Here are some flow charts for you:
// 1. Run in task flow: DuskServer.run -> DuskServer.run_inside_task -> process_task -> Process.bootstrap -> Process.main
// 2. Run in session flow: Process.run -> Process.bootstrap -> Process.main
//
// Note: Never run process.main directly, always use process.bootstrap.

#[async_trait::async_trait(?Send)]
pub trait Process {
    fn pid(&self) -> u64;
    fn program_id(&self) -> u64;
    fn namespace(&self) -> Rc<Namespace>;
    fn clone_box(&self) -> Box<dyn Process>;
    fn portal(&self) -> portal::Client;

    async fn signal(&self, _signal: Signal) -> Result<()> {
        Ok(())
    }

    async fn main(&self) -> Result<()> {
        core::future::pending().await
    }
}

impl dyn Process {
    pub async fn bootstrap(&self) -> Result<()> {
        let namespace = self.namespace();
        let mut ps_map = namespace.ps_map.lock().await;
        ps_map.insert(self.pid(), self.clone_box());
        debug!(
            "Process with pid {} registered in namespace {}",
            self.pid(),
            namespace.id
        );
        let result = self.main().await;
        ps_map.remove(&self.pid());
        debug!(
            "Process with pid {} unregistered from namespace {}",
            self.pid(),
            namespace.id
        );
        result
    }
}

impl process::Server for dyn Process {
    fn pid(
        &mut self,
        _params: process::PidParams,
        mut results: process::PidResults,
    ) -> Promise<(), ::capnp::Error> {
        results.get().set_result(Process::pid(self));

        Promise::ok(())
    }

    fn program_id(
        &mut self,
        _params: process::ProgramIdParams,
        mut results: process::ProgramIdResults,
    ) -> Promise<(), ::capnp::Error> {
        results.get().set_result(Process::program_id(self));

        Promise::ok(())
    }

    /// Kill can be used to send a signal to the process.
    /// If the `Signal::Kill`` is sent, the process is force to terminate the next time the future of main yields.
    /// If the `Signal::Terminate` is sent, the process is allowed to clean up before exiting.
    fn kill(
        &mut self,
        params: process::KillParams,
        mut _results: process::KillResults,
    ) -> Promise<(), ::capnp::Error> {
        let signal = match params.get() {
            Ok(p) => p.get_signal(),
            Err(e) => return Promise::err(capnp::Error::failed(e.to_string())),
        };

        let process = self.clone_box();
        Promise::from_future(async move {
            (*process)
                .signal(signal.into())
                .await
                .map_err(|e| capnp::Error::failed(e.to_string()))
        })
    }

    fn portal(
        &mut self,
        _params: process::PortalParams,
        mut results: process::PortalResults,
    ) -> Promise<(), ::capnp::Error> {
        let portal = <Self as Process>::portal(self);
        results.get().set_result(portal);
        Promise::ok(())
    }

    fn run(
        &mut self,
        _params: process::RunParams,
        mut _results: process::RunResults,
    ) -> Promise<(), ::capnp::Error> {
        let process = self.clone_box();
        Promise::from_future(async move {
            process
                .bootstrap()
                .await
                .map_err(|e| capnp::Error::failed(e.to_string()))
        })
    }
}

impl process::Server for Box<dyn Process> {
    fn pid(
        &mut self,
        params: process::PidParams,
        results: process::PidResults,
    ) -> Promise<(), ::capnp::Error> {
        <dyn Process as process::Server>::pid(&mut **self, params, results)
    }

    fn program_id(
        &mut self,
        params: process::ProgramIdParams,
        results: process::ProgramIdResults,
    ) -> Promise<(), ::capnp::Error> {
        <dyn Process as process::Server>::program_id(&mut **self, params, results)
    }

    fn portal(
        &mut self,
        params: process::PortalParams,
        results: process::PortalResults,
    ) -> Promise<(), ::capnp::Error> {
        <dyn Process as process::Server>::portal(&mut **self, params, results)
    }

    fn kill(
        &mut self,
        params: process::KillParams,
        results: process::KillResults,
    ) -> Promise<(), ::capnp::Error> {
        <dyn Process as process::Server>::kill(&mut **self, params, results)
    }

    fn run(
        &mut self,
        params: process::RunParams,
        results: process::RunResults,
    ) -> Promise<(), ::capnp::Error> {
        <dyn Process as process::Server>::run(&mut **self, params, results)
    }
}
