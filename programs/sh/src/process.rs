use crate::sh_capnp;
use dusk_program::Process;
use slab::Slab;
use log::debug;

pub struct ShProcess {
    pid: u64,
    matchers: Slab<fn(&str) -> bool>,
}

impl ShProcess {
    pub fn new(pid: u64, matchers: Slab<fn(&str) -> bool>) -> ShProcess {
        debug!("sh process created with pid {}", pid);
        ShProcess { pid, matchers }
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

    fn main(&self) -> capnp::capability::Promise<(), capnp::Error> {
        debug!("sh process with pid {} main called", self.pid);
        capnp::capability::Promise::ok(())
    }
}
