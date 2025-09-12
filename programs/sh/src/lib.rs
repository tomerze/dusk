pub mod args;
pub mod launcher;
pub mod portal;
pub mod process;

extern crate alloc;

#[allow(clippy::all)]
pub mod sh_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/sh_capnp.rs"));
}
