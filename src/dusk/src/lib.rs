#![feature(type_alias_impl_trait)]
#![feature(prelude_import)]
#![no_std]

extern crate alloc;

mod prelude;

// This allows us to essentially completely override rust's default prelude with our own.
// We do this to bring in things captnc depends on, like `Box`.
#[allow(unused)]
#[prelude_import]
use prelude::*;

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

use capnp_rpc::{rpc_twoparty_capnp, twoparty, RpcSystem};

use alloc::sync::Arc;
use log::{error, info};

use core::pin::Pin;
use futures::{AsyncRead, AsyncWrite};

pub mod driver;
pub mod namespace;
pub mod server;

pub mod dusk_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/dusk_capnp.rs"));
}

#[embassy_executor::task]
async fn rpc_system_wrapper(rpc_system: RpcSystem<rpc_twoparty_capnp::Side>) {
    if let Err(err) = rpc_system.await {
        error!("an error occured in an rpc system: {err}");
    }
}

#[embassy_executor::task]
pub async fn session(
    namespace: Arc<Mutex<CriticalSectionRawMutex, namespace::Namespace>>,
    reader: Pin<Box<dyn AsyncRead>>,
    writer: Pin<Box<dyn AsyncWrite>>,
) {
    info!("session started");

    let dusk_client: dusk_capnp::dusk::Client =
        capnp_rpc::new_client(server::DuskImpl::new(namespace));

    let network = twoparty::VatNetwork::new(
        reader,
        writer,
        rpc_twoparty_capnp::Side::Server,
        Default::default(),
    );

    let rpc_system = RpcSystem::new(Box::new(network), Some(dusk_client.clone().client));

    let spawner = Spawner::for_current_executor().await;
    if let Err(err) = spawner.spawn(rpc_system_wrapper(rpc_system)) {
        error!("an error occured in an rpc system task: {err:#?}");
    }
}
