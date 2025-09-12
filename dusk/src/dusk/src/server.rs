use crate::driver;
use alloc::rc::Rc;
use alloc::string::String;
use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::dusk_capnp::process;
use dusk_capnp::pry;
use dusk_program::namespace::Namespace;
use dusk_program::process::Process;
use embassy_executor::Spawner;
use log::{debug, error};

pub struct DuskServer {
    namespace: Rc<Namespace>,
    spawner: Spawner,
}

impl DuskServer {
    pub fn new(namespace: Rc<Namespace>, spawner: Spawner) -> Self {
        DuskServer { namespace, spawner }
    }

    async fn run_inside_task(
        process_client: process::Client,
        namespace: Rc<Namespace>,
        spawner: Spawner,
    ) -> Result<(), capnp::Error> {
        let ps_server_set = namespace.ps_server_set.lock().await;

        if let Some(process_server) = ps_server_set.get_local_server(&process_client).await {
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
        namespace: Rc<Namespace>,
    ) -> Result<process::Client, capnp::Error> {
        let mut ps_server_set = namespace.ps_server_set.lock().await;
        // Creates a new client from the server and append the server to the set
        let client = ps_server_set.new_client(process);
        Ok(client)
    }
}

#[embassy_executor::task]
async fn process_task(process: Box<dyn Process>) {
    embassy_futures::yield_now().await;
    if let Err(err) = process.bootstrap().await {
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

        let namespace = self.namespace.clone();
        Promise::from_future(async move {
            let process = driver::process(namespace.clone(), program_args)
                .await
                .map_err(|e| capnp::Error::failed(e.to_string()))?;

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

        Promise::from_future(Self::run_inside_task(
            process,
            self.namespace.clone(),
            self.spawner,
        ))
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
        let hostname = match driver::hostname() {
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
