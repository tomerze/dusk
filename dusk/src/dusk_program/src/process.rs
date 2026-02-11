use alloc::{boxed::Box, rc::Rc, string::ToString};
use anyhow::Result;
use dusk_capnp::capnp;
use dusk_capnp::capnp::capability::Promise;
use dusk_capnp::dusk_capnp::portal;
use dusk_capnp::dusk_capnp::process;
use embassy_sync::channel::DynamicReceiver;

use crate::namespace::Namespace;
use crate::namespace::SignalChannel;
use crate::signal;

#[async_trait::async_trait(?Send)]
pub trait ProcessMixin {
    fn portal(&self) -> portal::Client;
    async fn main(
        &self,
        signal_receiver: DynamicReceiver<'async_trait, signal::Signal>,
    ) -> Result<()>;
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
// LauncherSet.launch <- program_args.get_program_id (dusk server back to dusk client RPC)
// -> Launcher.launch -> Process
// Cool right?
//
// Note: Never run process.main directly, always use process.bootstrap.
#[async_trait::async_trait(?Send)]
pub trait Process: ProcessMixin {
    fn pid(&self) -> u64;
    fn program_id(&self) -> u64;
    fn name(&self) -> alloc::string::String;
    fn version(&self) -> alloc::string::String;
    fn namespace(&self) -> Rc<Namespace>;
    fn clone_box(&self) -> Box<dyn Process>;

    fn portal(&self) -> portal::Client {
        <Self as ProcessMixin>::portal(self)
    }
}

impl dyn Process {
    pub async fn bootstrap(&self) -> Result<()> {
        let namespace = self.namespace();
        {
            let mut ps_map = namespace.ps_map.lock().await;

            if ps_map.contains_key(&self.pid()) {
                return Err(anyhow::anyhow!(
                    "pid {} is already in use in namespace {}",
                    self.pid(),
                    namespace.id
                ));
            }

            ps_map.insert(self.pid(), self.clone_box());
        }
        let channel = Rc::new(SignalChannel::new());
        let signal_receiver = channel.dyn_receiver();
        {
            let mut ps_signal_channel_map = namespace.ps_signal_channel_map.lock().await;
            ps_signal_channel_map.insert(self.pid(), channel.clone());
        }
        log::debug!(
            "started process with pid {}, program id {}",
            self.pid(),
            self.program_id()
        );
        let result = self.main(signal_receiver).await;
        log::debug!(
            "exited process with pid {}, program id {}",
            self.pid(),
            self.program_id()
        );
        {
            let mut ps_signal_channel_map = namespace.ps_signal_channel_map.lock().await;
            ps_signal_channel_map.remove(&self.pid());
        }
        {
            let mut ps_map = namespace.ps_map.lock().await;
            ps_map.remove(&self.pid());
        }
        result
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
            let is_process_running = {
                let ps_map = namespace.ps_map.lock().await;
                ps_map.contains_key(&pid)
            };
            if is_process_running {
                let portal = <Self as Process>::portal(&*process);
                results.get().set_result(portal);
                Ok(())
            } else {
                Err(capnp::Error::failed(
                    "process is not running, cannot get portal".to_string(),
                ))
            }
        })
    }

    fn run(
        &mut self,
        _params: process::RunParams,
        mut _results: process::RunResults,
    ) -> Promise<(), capnp::Error> {
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

#[macro_export]
macro_rules! basic_process {
    (
        $process_type:ident,
        $program_id:expr,
        $name:expr,
        $version:expr,
        $portal_type:path,
        $portal_client_type:path,
        $arg_type:path
    ) => {
        #[derive(Clone)]
        pub struct $process_type {
            pub pid: u64,
            pub namespace: alloc::rc::Rc<dusk_program::namespace::Namespace>,
            pub program_args: $arg_type,
        }

        impl $process_type {
            pub fn new(
                pid: u64,
                namespace: alloc::rc::Rc<dusk_program::namespace::Namespace>,
                program_args: $arg_type,
            ) -> Self {
                $process_type {
                    pid,
                    namespace,
                    program_args,
                }
            }
        }

        #[async_trait::async_trait(?Send)]
        impl dusk_program::process::ProcessMixin for $process_type {
            fn portal(&self) -> dusk_capnp::dusk_capnp::portal::Client {
                let client: $portal_client_type =
                    capnp_rpc::new_client(<$portal_type>::new(self.clone()));
                client.cast_to::<dusk_capnp::dusk_capnp::portal::Client>()
            }

            async fn main(
                &self,
                signal_receiver: embassy_sync::channel::DynamicReceiver<
                    'async_trait,
                    dusk_program::signal::Signal,
                >,
            ) -> anyhow::Result<()> {
                loop {
                    let signal = signal_receiver.receive().await;
                    match signal {
                        dusk_program::signal::Signal::Terminate => return Ok(()),
                        dusk_program::signal::Signal::Unknown(_signal) => {}
                    }
                }
            }
        }

        #[async_trait::async_trait(?Send)]
        impl dusk_program::process::Process for $process_type {
            fn pid(&self) -> u64 {
                self.pid
            }
            fn program_id(&self) -> u64 {
                $program_id
            }
            fn name(&self) -> alloc::string::String {
                alloc::string::String::from($name)
            }
            fn version(&self) -> alloc::string::String {
                alloc::string::String::from($version)
            }
            fn namespace(&self) -> alloc::rc::Rc<dusk_program::namespace::Namespace> {
                self.namespace.clone()
            }
            fn clone_box(&self) -> Box<dyn dusk_program::process::Process> {
                Box::new($process_type {
                    pid: self.pid,
                    namespace: self.namespace.clone(),
                    program_args: self.program_args.clone(),
                })
            }
        }
    };
}
