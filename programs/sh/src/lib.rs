use anyhow::Result;
use async_trait::async_trait;
use dusk_program::{Launcher, Process};

pub mod sh_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/sh_capnp.rs"));
}

struct ShLauncher {}

impl ShLauncher {
    fn new() -> Self {
        ShLauncher {}
    }
}

#[async_trait]
impl Launcher for ShLauncher {
    async fn launch(&mut self) -> Result<Box<dyn Process>> {}
}
