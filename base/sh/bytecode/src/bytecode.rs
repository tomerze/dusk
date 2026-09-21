use alloc::vec::Vec;

use capnp::message::{Builder, HeapAllocator, Reader, ReaderOptions};
use capnp::serialize::OwnedSegments;

use crate::parser::Parser;
use crate::bytecode_capnp::script;
use crate::syntax_error::SyntaxError;

pub fn lower_from_source(source: &str) -> Result<Vec<u8>, SyntaxError> {
    let mut message = Builder::new(HeapAllocator::new());
    Parser::new().parse(source, message.init_root::<script::Builder<'_>>())?;
    Ok(capnp::serialize::write_message_to_words(&message))
}

pub fn read(bytecode: &[u8]) -> capnp::Result<Reader<OwnedSegments>> {
    capnp::serialize::read_message(bytecode, ReaderOptions::new())
}
