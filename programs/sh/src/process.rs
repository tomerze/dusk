use crate::portal::ShPortal;
use anyhow::Result;
use async_trait::async_trait;
use dusk_program::Process;
use slab::Slab;

pub struct ShProcess {
    matchers: Slab<fn(&str) -> bool>,
}

impl ShProcess {
    pub fn new(matchers: Slab<fn(&str) -> bool>) -> ShProcess {
        ShProcess { matchers }
    }
}

#[async_trait]
impl<'a> Process<'a> for ShProcess {
    async fn name(&self) -> String {
        String::from("sh")
    }

    async fn portal(&'a self) -> Result<Box<dyn dusk_capnp::dusk_capnp::portal::Server + 'a>> {
        Ok(Box::new(ShPortal::new(self)))
    }
}
