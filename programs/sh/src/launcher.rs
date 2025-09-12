use alloc::rc::Rc;
use anyhow::Result;
use dusk_program::{launcher::Launcher, namespace::Namespace, process::Process};

use slab::Slab;

use crate::process::ShProcess;

pub struct ShLauncher {
    matchers: Slab<fn(&str) -> bool>,
}

impl Default for ShLauncher {
    fn default() -> Self {
        Self::new()
    }
}

impl ShLauncher {
    pub fn new() -> Self {
        ShLauncher {
            matchers: Slab::new(),
        }
    }

    pub fn matchers(&self) -> &Slab<fn(&str) -> bool> {
        &self.matchers
    }

    pub fn matchers_mut(&mut self) -> &mut Slab<fn(&str) -> bool> {
        &mut self.matchers
    }
}

impl Launcher for ShLauncher {
    fn launch(&mut self, pid: u64, namespace: Rc<Namespace>) -> Result<Box<dyn Process>> {
        Ok(Box::new(ShProcess::new(
            pid,
            namespace.clone(),
            self.matchers.clone(),
        )))
    }
}
