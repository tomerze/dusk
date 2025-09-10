use crate::driver;
use crate::namespace::Namespace;
use alloc::string::String;
use alloc::sync::Arc;
use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::pry;
use log::error;

pub struct DuskServer {
    namespace: Arc<Namespace>,
}

impl DuskServer {
    pub fn new(namespace: Arc<Namespace>) -> Self {
        DuskServer { namespace }
    }
}

impl dusk::Server for DuskServer {
    fn process(
        &mut self,
        params: dusk::ProcessParams,
        mut results: dusk::ProcessResults,
    ) -> Promise<(), ::capnp::Error> {
        let program_args = pry!(pry!(params.get()).get_program_args());
        let process = pry!(driver::create_process(self.namespace.id, program_args)
            .map_err(|err| capnp::Error::failed(err.to_string())));
        results.get().set_result(capnp_rpc::new_client(process));
        Promise::ok(())
    }

    fn run(
        &mut self,
        params: dusk::RunParams,
        mut _results: dusk::RunResults,
    ) -> Promise<(), ::capnp::Error> {
        // TODO: actually run the process
        Promise::ok(())
    }

    fn portal(
        &mut self,
        params: dusk::PortalParams,
        results: dusk::PortalResults,
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
        let process_entries = results.get().init_process_entries(0);
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
