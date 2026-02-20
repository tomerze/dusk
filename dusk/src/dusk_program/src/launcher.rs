use crate::{process::Process, process::ProcessContext};
use alloc::boxed::Box;
use anyhow::Result;

pub trait Launcher: LauncherMixin {
    fn program_id(&self) -> u64;
}

#[async_trait::async_trait(?Send)]
pub trait LauncherMixin {
    async fn launch(&mut self, process_context: ProcessContext) -> Result<Box<dyn Process>>;
}
