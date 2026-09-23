use capnp::message::{Reader, ReaderOptions};
use capnp::serialize::OwnedSegments;

pub fn read(bytecode: &[u8]) -> capnp::Result<Reader<OwnedSegments>> {
    capnp::serialize::read_message(bytecode, ReaderOptions::new())
}
