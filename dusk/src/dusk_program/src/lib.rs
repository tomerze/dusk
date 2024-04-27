#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait Process {
    fn pid(&self) -> u64;
    fn program_id(&self) -> u64;

    async fn main(&self /* Get channel of signals here */) -> Result<()> {
        let future = futures::future::pending();
        let () = future.await;
        Ok(())
    }
}

#[async_trait]
pub trait Launcher {
    async fn launch(&mut self, pid: u64) -> Result<Box<dyn Process>>;
}
