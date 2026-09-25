use alloc::vec::Vec;

use capnp::message::{Builder, HeapAllocator};

use crate::bytecode_capnp::script;
use crate::parser::Parser;
use crate::syntax_error::SyntaxError;

pub fn compile(source: &str) -> Result<Vec<u8>, SyntaxError> {
    let mut message = Builder::new(HeapAllocator::new());
    Parser::new().parse(source, message.init_root::<script::Builder<'_>>())?;
    Ok(capnp::serialize::write_message_to_words(&message))
}
