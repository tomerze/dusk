use std::borrow::Cow;

use anyhow::Result;
use async_trait::async_trait;
use dusk_program::{Launcher, Process};
use slab::Slab;

pub mod sh_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/sh_capnp.rs"));
}

trait Plugin {
    fn r#match(self, program: &str) -> bool;
}

struct ShLauncher {
    plugins: Slab<Box<dyn Plugin>>,
}

impl ShLauncher {
    fn new() -> Self {
        ShLauncher {
            plugins: Slab::new(),
        }
    }

    fn plugins(&self) -> &Slab<Box<dyn Plugin>> {
        &self.plugins
    }

    fn plugins_mut(&mut self) -> &mut Slab<Box<dyn Plugin>> {
        &mut self.plugins
    }
}

#[async_trait]
impl Launcher for ShLauncher {
    async fn launch(&mut self) -> Result<Box<dyn Process>> {}
}

struct ShProcess {}

#[async_trait]
impl Process for ShProcess {
    async fn name(&self) -> String {
        String::from("sh")
    }

    async fn portal(&self) -> Result<Box<dyn dusk_capnp::dusk_capnp::portal::Server>> {

    }
}

use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::dusk;

pub struct ShPortal {
}

impl ShPortal {
    pub fn new() -> Self {
        ShPortal {}
    }
}

impl dusk::Server for DuskImpl {
    fn exec(
        &mut self,
        _params: dusk::ExecParams,
        mut _results: dusk::ExecResults,
    ) -> Promise<(), ::capnp::Error> {
        Promise::ok(())
    }
