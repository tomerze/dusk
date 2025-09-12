use crate::sh_capnp;
use alloc::rc::Rc;
use anyhow::Result;
use dusk_program::{namespace::Namespace, process::Process};
use embassy_time::Timer;
use slab::Slab;

pub struct ShProcess {
    pid: u64,
    namespace: Rc<Namespace>,
    _matchers: Slab<fn(&str) -> bool>,
}

impl ShProcess {
    pub fn new(pid: u64, namespace: Rc<Namespace>, _matchers: Slab<fn(&str) -> bool>) -> ShProcess {
        ShProcess {
            pid,
            namespace,
            _matchers,
        }
    }
}

impl Drop for ShProcess {
    fn drop(&mut self) {}
}

#[async_trait::async_trait(?Send)]
impl Process for ShProcess {
    fn pid(&self) -> u64 {
        self.pid
    }
    fn program_id(&self) -> u64 {
        sh_capnp::PROGRAM_ID
    }

    fn namespace(&self) -> Rc<Namespace> {
        self.namespace.clone()
    }

    fn clone_box(&self) -> Box<dyn Process> {
        Box::new(ShProcess {
            pid: self.pid,
            namespace: self.namespace.clone(),
            _matchers: self._matchers.clone(),
        })
    }

    async fn main(&self) -> Result<()> {
        Timer::after_secs(5).await;
        Ok(())
    }
}
