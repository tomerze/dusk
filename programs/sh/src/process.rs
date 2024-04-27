use crate::portal::ShPortal;
use anyhow::Result;
use async_trait::async_trait;
use dusk_capnp::dusk_capnp::portal;
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
    fn name(&self) -> String {
        String::from("sh")
    }

    fn portal(&'a self) -> Result<portal::Client> {
        let x = ShPortal::new(&self);
        let y = capnp_rpc::new_client(x);
        Ok(y)
    }
}
