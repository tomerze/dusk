//! Common imports for programs using dusk_program
#![allow(exported_private_dependencies)]

// Re-export entire crates
pub use anyhow;
pub use async_trait;
pub use capnp;
pub use capnp_rpc;
pub use embassy_executor;
pub use embassy_sync;
pub use futures;

// Re-export commonly used types for convenience
pub use capnp::capability::{FromClientHook, Promise};
pub use embassy_executor::Spawner;
pub use embassy_sync::channel::DynamicReceiver;

// Re-export specific futures traits that are commonly needed
pub use futures::io::{AsyncReadExt, AsyncWriteExt};
pub use futures::FutureExt;

// Re-export commonly used dusk_program modules and types
pub use crate::portal::Portal;
pub use crate::signal::{self, Signal};
pub use crate::{basic_launcher, basic_process, impl_portal_server, impl_program_args_server};
