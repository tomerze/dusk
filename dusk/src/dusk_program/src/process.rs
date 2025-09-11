use alloc::boxed::Box;
use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::process;

pub trait Process {
    fn pid(&self) -> u64;
    fn program_id(&self) -> u64;
    fn clone_box(&self) -> Box<dyn Process>;

    fn main(&self) -> Promise<(), ::capnp::Error> {
        Promise::ok(())
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

    fn run(
        &mut self,
        _params: process::RunParams,
        mut _results: process::RunResults,
    ) -> Promise<(), ::capnp::Error> {
        self.main()
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

    fn run(
        &mut self,
        params: process::RunParams,
        results: process::RunResults,
    ) -> Promise<(), ::capnp::Error> {
        <dyn Process as process::Server>::run(&mut **self, params, results)
    }
}
