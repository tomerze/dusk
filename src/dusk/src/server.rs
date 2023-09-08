use crate::dusk_capnp::dusk;

use capnp::capability::Promise;

pub struct DuskImpl;

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
