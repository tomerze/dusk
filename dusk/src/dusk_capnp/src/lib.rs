#[allow(clippy::all)]
extern crate alloc;

pub mod prelude;

pub mod stream_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/stream_capnp.rs"));
}

#[allow(clippy::all)]
pub mod dusk_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/dusk_capnp.rs"));
}

pub static DUSK_SCHEMA: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/capnp/dusk.capnp"));

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
                return ::capnp::capability::Promise::err(::core::convert::From::from(err))
            }
        }
    };
}
