#![no_std]

extern crate alloc;

#[cfg(feature = "parser")]
pub mod ast;
#[cfg(feature = "parser")]
pub mod parser;
#[cfg(feature = "parser")]
pub mod syntax_error;
#[cfg(feature = "parser")]
pub mod tokenize;

#[allow(clippy::all)]
pub mod bytecode_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/bytecode_capnp.rs"));
}
