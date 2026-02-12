//! Common imports for programs using dusk_program
#![allow(exported_private_dependencies)]

// Re-export entire crates
pub use anyhow;
pub use async_trait;
pub use dusk_capnp::capnp;
pub use dusk_capnp::capnp_rpc;
pub use embassy_executor;
pub use embassy_sync;
pub use futures;

// Re-export commonly used types for convenience
pub use dusk_capnp::capnp::capability::{FromClientHook, Promise};
pub use dusk_capnp::dusk_capnp::dusk;
pub use embassy_executor::Spawner;
pub use embassy_sync::channel::DynamicReceiver;

// Re-export specific futures traits that are commonly needed
pub use futures::FutureExt;
pub use futures::io::{AsyncReadExt, AsyncWriteExt};

// Re-export commonly used dusk_program modules and types

pub use crate::process::{Process as _, ProcessContext, ProcessMixin};
pub use crate::signal::{self, Signal};
