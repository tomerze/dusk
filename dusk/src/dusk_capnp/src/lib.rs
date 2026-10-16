#![no_std]
#![allow(internal_features)]
#![feature(prelude_import)]

#[allow(clippy::all)]
extern crate alloc;

/// The version of the `dusk_capnp` crate (read from Cargo.toml at compile time).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The git revision of the workspace at build time.
pub const GIT_REV: &str = env!("GIT_REV");

// Re-export Cap'n Proto crates for all dependent crates
pub extern crate capnp;
pub extern crate capnp_futures;
pub extern crate capnp_rpc;

pub mod prelude;

#[allow(unused)]
#[prelude_import]
use crate::prelude::*;

pub mod stream_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/stream_capnp.rs"));
}

#[allow(clippy::all)]
pub mod dusk_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/dusk_capnp.rs"));
}

pub const fn capnp_bin_path() -> &'static str {
    match option_env!("DUSK_CAPNP_BIN_PATH") {
        Some(path) => path,
        None => panic!("DUSK_CAPNP_BIN_PATH is not set; ensure dusk_capnp's build script ran"),
    }
}

/// Like capnp_rpc's `pry!()`, but supports `no_std`
#[macro_export]
macro_rules! pry {
    ($expr:expr) => {
        match $expr {
            ::core::result::Result::Ok(val) => val,
            ::core::result::Result::Err(err) => {
                return $crate::capnp::capability::Promise::err(::core::convert::From::from(err))
            }
        }
    };
}

pub struct CapnpDep {
    /// Path to the `.capnp` source file (relative to workspace root or absolute).
    pub schema: &'static str,
    /// The Rust crate that provides the generated code for this schema.
    pub crate_name: &'static str,
    /// The top-level schema IDs contained in this file.
    pub schema_ids: &'static [u64],
}

/// The `dusk.capnp` schema, always available as a dependency.
pub const DUSK_CAPNP: CapnpDep = CapnpDep {
    schema: concat!(env!("CARGO_MANIFEST_DIR"), "/capnp/dusk.capnp"),
    crate_name: "dusk_capnp",
    schema_ids: &[0xace6963097d486d6],
};

/// The `stream.capnp` schema, always available as a dependency.
pub const STREAM_CAPNP: CapnpDep = CapnpDep {
    schema: concat!(env!("CARGO_MANIFEST_DIR"), "/capnp/stream.capnp"),
    crate_name: "dusk_capnp",
    schema_ids: &[0x86c366a91393f3f8],
};
