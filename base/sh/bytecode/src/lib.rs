#![no_std]

extern crate alloc;

pub mod ast;
pub mod tokenize;

#[allow(clippy::all)]
pub mod bytecode_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/bytecode_capnp.rs"));
}
