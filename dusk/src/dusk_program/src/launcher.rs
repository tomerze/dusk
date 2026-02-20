use crate::{process::Process, process::ProcessContext};
use alloc::boxed::Box;
use anyhow::Result;

pub trait Launcher: LauncherMixin {
    fn program_id(&self) -> u64;
}

pub trait LauncherMixin {
    fn launch(&mut self, process_context: ProcessContext) -> Result<Box<dyn Process>>;
}
