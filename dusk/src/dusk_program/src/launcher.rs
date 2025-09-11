use crate::process::Process;
use alloc::boxed::Box;
use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait Launcher {
    async fn launch(&mut self, pid: u64) -> Result<Box<dyn Process>>;
}
