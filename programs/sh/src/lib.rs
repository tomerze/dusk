pub mod stream_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/stream_capnp.rs"));
}

pub mod sh_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/sh_capnp.rs"));
}
