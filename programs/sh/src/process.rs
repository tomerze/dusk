use crate::sh_capnp;
use async_trait::async_trait;
use dusk_program::Process;
use slab::Slab;

pub struct ShProcess {
    pid: u64,
    matchers: Slab<fn(&str) -> bool>,
}

impl ShProcess {
    pub fn new(pid: u64, matchers: Slab<fn(&str) -> bool>) -> ShProcess {
        ShProcess { pid, matchers }
    }
}

#[async_trait]
impl Process for ShProcess {
    fn pid(&self) -> u64 {
        self.pid
    }
    fn program_id(&self) -> u64 {
        sh_capnp::PROGRAM_ID
    }
}
