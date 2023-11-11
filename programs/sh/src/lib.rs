pub mod args;
pub mod launcher;
pub mod portal;
pub mod process;

pub mod sh_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/sh_capnp.rs"));
}
