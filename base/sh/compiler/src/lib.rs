#![no_std]

extern crate alloc;

use capnp::message::{Reader, ReaderOptions};
use capnp::serialize::OwnedSegments;

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

pub type BytecodeMessage =
    dusk_capnp::capnp_rpc::ImbuedMessageBuilder<capnp::message::HeapAllocator>;

pub fn read(bytecode: &[u8]) -> capnp::Result<Reader<OwnedSegments>> {
    capnp::serialize::read_message(bytecode, ReaderOptions::new())
}
