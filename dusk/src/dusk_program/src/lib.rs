#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait Process<'a> {
    async fn name(&self) -> String;
    async fn main(&self /* Get channel of signals here */) -> Result<()> {
        let future = futures::future::pending();
        let () = future.await;
        Ok(())
    }
    async fn portal(&'a self) -> Result<Box<dyn dusk_capnp::dusk_capnp::portal::Server + 'a>>;
}

#[async_trait]
pub trait Launcher {
    async fn launch(&mut self) -> Result<Box<dyn Process>>;
}
