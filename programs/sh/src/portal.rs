use crate::process::ShProcess;
use crate::sh_capnp::sh_portal;
use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::portal;

pub struct ShPortal<'a> {
    _process: &'a ShProcess,
}

impl<'a> ShPortal<'a> {
    pub fn new(_process: &'a ShProcess) -> Self {
        ShPortal { _process }
    }
}

impl<'a> portal::Server for ShPortal<'a> {
    fn process(
        &mut self,
        _: portal::ProcessParams,
        _: portal::ProcessResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        Promise::ok(())
    }
}

impl<'a> sh_portal::Server for ShPortal<'a> {
    fn set_env(
        &mut self,
        _: sh_portal::SetEnvParams,
        _: sh_portal::SetEnvResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        Promise::ok(())
    }
    fn get_env(
        &mut self,
        _: sh_portal::GetEnvParams,
        _: sh_portal::GetEnvResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        Promise::ok(())
    }
    fn sh(
        &mut self,
        _: sh_portal::ShParams,
        _: sh_portal::ShResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        Promise::ok(())
    }
}
