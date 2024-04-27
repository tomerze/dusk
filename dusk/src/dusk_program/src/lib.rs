#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use anyhow::Result;
use async_trait::async_trait;
use dusk_capnp::dusk_capnp::portal;

#[async_trait]
pub trait Process<'a> {
    fn name(&self) -> String;
    fn portal(&'a self) -> Result<&'a portal::Client>;

    async fn main(&self /* Get channel of signals here */) -> Result<()> {
        let future = futures::future::pending();
        let () = future.await;
        Ok(())
    }
}

#[async_trait]
pub trait Launcher {
    async fn launch(&mut self) -> Result<Box<dyn Process>>;
}
