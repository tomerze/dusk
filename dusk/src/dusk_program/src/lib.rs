#![no_std]

extern crate alloc;

// Re-export public dependencies for prelude
pub extern crate anyhow;
pub extern crate async_trait;
pub extern crate capnp;
pub extern crate capnp_rpc;
pub extern crate embassy_executor;
pub extern crate embassy_sync;
pub extern crate futures; // Not pub extern because we selectively re-export parts

pub mod launcher;
pub mod launcher_set;
pub mod namespace;
pub mod portal;
pub mod prelude;
pub mod process;
pub mod program_args;
pub mod signal;
pub mod stream;
pub mod value;
