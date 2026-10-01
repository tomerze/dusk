#![no_std]

extern crate alloc;

// Import capnp at the crate root for generated code that uses ::capnp:: paths
pub extern crate dusk_capnp;

// Re-export public dependencies for prelude
pub extern crate anyhow;
pub extern crate async_trait;
pub extern crate embassy_executor;
pub extern crate embassy_futures;
pub extern crate embassy_sync;
pub extern crate embassy_time;
pub extern crate futures;
pub extern crate hashbrown;

pub mod handle;
pub mod launcher;
pub mod launcher_set;
pub mod namespace;
pub mod prelude;
pub mod process;
pub mod program_args;
pub mod ready;
pub mod signal;
pub mod stream;
pub mod value;

/// Extension trait for converting error results into `capnp::Error`.
///
/// For `anyhow::Error`, uses `{:#}` formatting to preserve the full error chain.
/// For other `Display` errors, uses standard formatting.
pub trait IntoCapnp<T> {
    fn into_capnp(self) -> Result<T, dusk_capnp::capnp::Error>;
}

impl<T> IntoCapnp<T> for anyhow::Result<T> {
    fn into_capnp(self) -> Result<T, dusk_capnp::capnp::Error> {
        self.map_err(|e| dusk_capnp::capnp::Error::failed(alloc::format!("{e:?}")))
    }
}
