use crate::dusk_capnp::dusk;
use crate::namespace::Namespace;
use alloc::sync::Arc;
use capnp::capability::Promise;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

pub struct DuskImpl {
    namespace: Arc<Mutex<CriticalSectionRawMutex, Namespace>>,
}

impl DuskImpl {
    pub fn new(namespace: Arc<Mutex<CriticalSectionRawMutex, Namespace>>) -> Self {
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
        mut _results: dusk::HostnameResults,
    ) -> Promise<(), ::capnp::Error> {
        Promise::ok(())
    }
}
