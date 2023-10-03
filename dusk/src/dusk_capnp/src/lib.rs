pub mod stream_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/stream_capnp.rs"));
}

pub mod dusk_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/dusk_capnp.rs"));
}

pub static DUSK_SCHEMA: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/capnp/dusk.capnp"));
