use crate::driver;
use crate::namespace::Namespace;
use alloc::string::String;
use alloc::sync::Arc;
use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::dusk;
use log::error;

pub struct DuskImpl {
    namespace: Arc<Namespace>,
}

impl DuskImpl {
    pub fn new(namespace: Arc<Namespace>) -> Self {
        DuskImpl { namespace }
    }
}

impl dusk::Server for DuskImpl {
    fn exec(
        &mut self,
        _params: dusk::ExecParams,
        mut _results: dusk::ExecResults,
    ) -> Promise<(), ::capnp::Error> {
        Promise::ok(())
    }

    fn kill(
        &mut self,
        _params: dusk::KillParams,
        mut _results: dusk::KillResults,
    ) -> Promise<(), ::capnp::Error> {
        Promise::ok(())
    }

    fn ps(
        &mut self,
        _params: dusk::PsParams,
        mut _results: dusk::PsResults,
    ) -> Promise<(), ::capnp::Error> {
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
        results.get().set_hostname(hostname[..].into());

        Promise::ok(())
    }
}
