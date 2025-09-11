use crate::sh_capnp;
use dusk_program::process::Process;
use log::debug;
use slab::Slab;

pub struct ShProcess {
    pid: u64,
    _matchers: Slab<fn(&str) -> bool>,
}

impl ShProcess {
    pub fn new(pid: u64, _matchers: Slab<fn(&str) -> bool>) -> ShProcess {
        debug!("sh process created with pid {}", pid);
        ShProcess { pid, _matchers }
    }
}

impl Drop for ShProcess {
    fn drop(&mut self) {
        debug!("sh process with pid {} dropped", self.pid);
    }
}

impl Process for ShProcess {
    fn pid(&self) -> u64 {
        self.pid
    }
    fn program_id(&self) -> u64 {
        sh_capnp::PROGRAM_ID
    }

    fn clone_box(&self) -> Box<dyn Process> {
        Box::new(ShProcess {
            pid: self.pid,
            _matchers: self._matchers.clone(),
        })
    }

    fn main(&self) -> capnp::capability::Promise<(), capnp::Error> {
        debug!("sh process with pid {} main called", self.pid);

        std::thread::sleep(std::time::Duration::from_secs(5));
        capnp::capability::Promise::ok(())
    }
}
