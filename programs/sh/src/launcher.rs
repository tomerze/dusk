use anyhow::Result;
use async_trait::async_trait;
use dusk_program::{Launcher, Process};
use slab::Slab;

use crate::process::ShProcess;

pub struct ShLauncher {
    matchers: Slab<fn(&str) -> bool>,
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

#[async_trait]
impl Launcher for ShLauncher {
    async fn launch(&mut self) -> Result<Box<dyn Process>> {
        Ok(Box::new(ShProcess::new(self.matchers.clone())))
    }
}
