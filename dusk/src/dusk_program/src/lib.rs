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

#[async_trait]
pub trait Initiator {
    async fn init(&mut self, args: dusk_capnp::dusk_capnp::program_args::Client);
}

// struct LinuxInitiator {
//     sh: ShLauncher,
//     ls: LsLauncher,
// }
//
// impl LinuxInitiator {
//     async fn new() -> Result<Self> {
//         sh = ShLauncher::new();
//         ls = LsLauncher::new(sh_launcher);
//
//         Launchers { sh, ls }
//     }
// }
//
// impl Initiator for LinuxInitiator {
//     async fn init(
//         &mut self,
//         args: dusk::dusk_capnp::program_args::Server,
//     ) -> Result<Box<dyn Process>>;
// }
