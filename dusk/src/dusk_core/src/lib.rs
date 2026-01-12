#![allow(internal_features)]
#![feature(type_alias_impl_trait)]
#![feature(prelude_import)]
#![feature(impl_trait_in_assoc_type)]
#![no_std]

extern crate alloc;

// This allows us to essentially completely override rust's default prelude with our own.
// We do this to bring in things capnp depends on, like `Box`.
#[allow(unused)]
#[prelude_import]
use dusk_capnp::prelude::*;

use dusk_program::embassy_executor::Spawner;

use dusk_capnp::capnp_rpc::{RpcSystem, rpc_twoparty_capnp, twoparty};

use alloc::rc::Rc;
use log::{error, info};

use core::pin::Pin;
use futures_io::{AsyncRead, AsyncWrite};

pub mod driver;
pub mod server;
pub use dusk_capnp;

mod tls;

#[embassy_executor::task(pool_size = 8)]
pub async fn session(
    namespace: Rc<dusk_program::namespace::Namespace>,
    reader: Pin<Box<dyn AsyncRead>>,
    writer: Pin<Box<dyn AsyncWrite>>,
) {
    info!("session started with namespace `{}`", namespace.id);

    let spawner = unsafe { Spawner::for_current_executor().await };
    let (tls_reader, tls_writer) = tls::wrap_with_tls(reader, writer).await;
    let dusk_client: dusk_capnp::dusk_capnp::dusk::Client =
        dusk_capnp::capnp_rpc::new_client(server::DuskServer::new(namespace, spawner));
    let network = twoparty::VatNetwork::new(
        tls_reader,
        tls_writer,
        rpc_twoparty_capnp::Side::Server,
        Default::default(),
    );

    let rpc_system = RpcSystem::new(Box::new(network), Some(dusk_client.clone().client));

    if let Err(err) = rpc_system.await {
        error!("an error occured in an rpc system: `{err}`");
    }
}
