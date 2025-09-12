use crate::{namespace::Namespace, process::Process};
use alloc::{boxed::Box, rc::Rc};
use anyhow::Result;
use dusk_capnp::dusk_capnp::program_args;

pub trait Launcher {
    fn program_id(&self) -> u64;
    fn launch(
        &mut self,
        pid: u64,
        namespace: Rc<Namespace>,
        program_args: program_args::Client,
    ) -> Result<Box<dyn Process>>;
}
