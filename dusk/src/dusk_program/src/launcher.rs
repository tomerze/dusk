use crate::{namespace::Namespace, process::Process};
use alloc::{boxed::Box, rc::Rc};
use anyhow::Result;

pub trait Launcher {
    fn launch(&mut self, pid: u64, namespace: Rc<Namespace>) -> Result<Box<dyn Process>>;
}
