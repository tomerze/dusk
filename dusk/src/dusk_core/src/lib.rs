#![allow(internal_features)]
#![feature(prelude_import)]
#![feature(impl_trait_in_assoc_type)]
#![no_std]

extern crate alloc;

// This allows us to essentially completely override rust's default prelude with our own.
// We do this to bring in things capnp depends on, like `Box`.
#[allow(unused)]
#[prelude_import]
use dusk_capnp::prelude::*;

use dusk_capnp::capnp_rpc::{RpcSystem, rpc_twoparty_capnp, twoparty};

use alloc::rc::Rc;
use tracing::{error, info};

use core::cell::Cell;
use core::pin::Pin;
use core::sync::atomic::Ordering;
use futures_io::{AsyncRead, AsyncWrite};
use portable_atomic::AtomicU64;
use tracing::Instrument;

pub mod driver;
pub mod init;
pub mod server;
pub mod trace;
pub use dusk_capnp;

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

pub async fn local_client(
    namespace: Rc<dusk_program::namespace::Namespace>,
) -> dusk_capnp::dusk_capnp::dusk::Client {
    let dusk_client: dusk_capnp::dusk_capnp::dusk::Client =
        dusk_capnp::capnp_rpc::new_client(server::DuskServer::new(namespace));
    dusk_client
}

#[embassy_executor::task(pool_size = 16)]
pub async fn session(
    task_id: Rc<Cell<u32>>,
    namespace: Rc<dusk_program::namespace::Namespace>,
    reader: Pin<Box<dyn AsyncRead>>,
    writer: Pin<Box<dyn AsyncWrite>>,
) {
    let session_id = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let span = tracing::info_span!(
        "session",
        ?session_id,
        __new_task_id__ = task_id.get(),
        namespace_id = namespace.id,
        pid = tracing::field::Empty,
        program_id = tracing::field::Empty,
        program_name = tracing::field::Empty,
        program_version = tracing::field::Empty,
    );

    span.in_scope(|| info!("session started"));

    let dusk_client = local_client(namespace).await;
    let network = twoparty::VatNetwork::new(
        reader,
        writer,
        rpc_twoparty_capnp::Side::Server,
        Default::default(),
    );

    let rpc_system = RpcSystem::new(Box::new(network), Some(dusk_client.clone().client));

    if let Err(err) = rpc_system.instrument(span.clone()).await {
        error!("an error occured in an rpc system: `{err:#?}`");
    }

    span.in_scope(|| info!("session ended"));
}
