use crate::driver;
use crate::namespace::Namespace;
use alloc::string::String;
use alloc::sync::Arc;
use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::dusk_capnp::process;
use dusk_capnp::pry;
use dusk_program::Process;
use embassy_executor::Spawner;
use log::{debug, error};

pub struct DuskServer {
    namespace: Arc<Namespace>,
    spawner: Spawner,
}

impl DuskServer {
    pub fn new(namespace: Arc<Namespace>, spawner: Spawner) -> Self {
        DuskServer { namespace, spawner }
    }

    async fn async_run(
        process_client: process::Client,
        namespace: Arc<Namespace>,
        spawner: Spawner,
    ) -> Result<(), capnp::Error> {
        let processes = namespace.processes.lock().await;

        if let Some(process_server) = processes.get_local_server(&process_client).await {
            let process = process_server.borrow().server.clone_box();
            let pid = process.pid();
            match spawner.spawn(process_task(process)) {
                Ok(()) => {
                    debug!("Spawned process task for pid {}", pid);
                    Ok(())
                }
                Err(e) => Err(capnp::Error::failed(e.to_string())),
            }
        } else {
            Err(capnp::Error::failed("Process not found".to_string()))
        }
    }

    async fn new_process_client(
        process: Box<dyn Process>,
        namespace: Arc<Namespace>,
    ) -> Result<process::Client, capnp::Error> {
        let mut processes = namespace.processes.lock().await;
        let client = processes.new_client(process);
        Ok(client)
    }
}

#[embassy_executor::task]
async fn process_task(process: Box<dyn Process>) {
    if let Err(err) = process.main().await {
        error!(
            "Process with pid {} exited with error: {}",
            process.pid(),
            err
        );
    }
}

impl dusk::Server for DuskServer {
    fn process(
        &mut self,
        params: dusk::ProcessParams,
        mut results: dusk::ProcessResults,
    ) -> Promise<(), ::capnp::Error> {
        let program_args = pry!(pry!(params.get()).get_program_args());
        let process = pry!(driver::process(self.namespace.id, program_args)
            .map_err(|err| capnp::Error::failed(err.to_string())));

        let namespace = self.namespace.clone();
        Promise::from_future(async move {
            let process_client = Self::new_process_client(process, namespace).await?;

            results.get().set_result(process_client);
            Ok(())
        })
    }

    fn run(
        &mut self,
        params: dusk::RunParams,
        mut _results: dusk::RunResults,
    ) -> Promise<(), ::capnp::Error> {
        let process = pry!(pry!(params.get()).get_process());

        Promise::from_future(Self::async_run(
            process,
            self.namespace.clone(),
            self.spawner,
        ))
    }

    fn portal(
        &mut self,
        params: dusk::PortalParams,
        _results: dusk::PortalResults,
    ) -> Promise<(), ::capnp::Error> {
        let _pid = pry!(params.get()).get_pid();
        // TODO: lookup process by pid and return portal
        // Placeholder: return an empty portal

        //results.get().set_result(capnp_rpc::new_client(portal));

        Promise::ok(())
    }

    fn kill(
        &mut self,
        params: dusk::KillParams,
        mut _results: dusk::KillResults,
    ) -> Promise<(), ::capnp::Error> {
        let _pid = pry!(params.get()).get_pid();
        // TODO: kill process by pid
        Promise::ok(())
    }

    fn ps(
        &mut self,
        _params: dusk::PsParams,
        mut results: dusk::PsResults,
    ) -> Promise<(), ::capnp::Error> {
        // TODO: get process list
        let _process_entries = results.get().init_process_entries(0);
        // Fill process_entries as needed
        Promise::ok(())
    }

    fn hostname(
        &mut self,
        _params: dusk::HostnameParams,
        mut results: dusk::HostnameResults,
    ) -> Promise<(), ::capnp::Error> {
        let hostname = match driver::hostname(self.namespace.id) {
            Ok(id) => id,
            Err(err) => {
                error!("failed to receive hostname from driver: `{err}`");
                String::from("error")
            }
        };
        results.get().set_result(&hostname[..]);
        Promise::ok(())
    }
}
