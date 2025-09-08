#![allow(internal_features)]
#![feature(type_alias_impl_trait)]
#![feature(prelude_import)]
#![feature(impl_trait_in_assoc_type)]
#![no_std]

extern crate alloc;

mod prelude;

// This allows us to essentially completely override rust's default prelude with our own.
// We do this to bring in things captnc depends on, like `Box`.
#[allow(unused)]
#[prelude_import]
use prelude::*;

use embassy_executor::Spawner;

use capnp_rpc::{rpc_twoparty_capnp, twoparty, RpcSystem};

use alloc::sync::Arc;
use log::{error, info};

use core::pin::Pin;
use futures::{AsyncRead, AsyncWrite};

pub mod driver;
pub mod namespace;
pub mod server;
pub use dusk_capnp;

mod tls;

#[embassy_executor::task(pool_size = 16)]
async fn rpc_system_wrapper(rpc_system: RpcSystem<rpc_twoparty_capnp::Side>) {
    if let Err(err) = rpc_system.await {
        error!("an error occured in an rpc system: `{err}`");
    }
}

#[embassy_executor::task]
pub async fn session(
    namespace: Arc<namespace::Namespace>,
    reader: Pin<Box<dyn AsyncRead>>,
    writer: Pin<Box<dyn AsyncWrite>>,
) {
    info!("session started with namespace `{}`", namespace.id);

    let (tls_reader, tls_writer) = tls::wrap_with_tls(reader, writer).await;
    let dusk_client: dusk_capnp::dusk_capnp::dusk::Client =
        capnp_rpc::new_client(server::DuskImpl::new(namespace));

    let network = twoparty::VatNetwork::new(
        tls_reader,
        tls_writer,
        rpc_twoparty_capnp::Side::Server,
        Default::default(),
    );

    let rpc_system = RpcSystem::new(Box::new(network), Some(dusk_client.clone().client));

    let spawner = unsafe { Spawner::for_current_executor().await };
    if let Err(err) = spawner.spawn(rpc_system_wrapper(rpc_system)) {
        error!("an error occured while spawning an rpc system task: {err:#?}");
    }
}
