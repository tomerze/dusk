//! The producer and consumer handles over a [`LogBuffer`](crate::buffer::LogBuffer):
//! the append-only [`Writer`] and the non-destructive [`Reader`]. Both share their
//! originating buffer's lanes; obtain them via
//! [`LogBuffer::writer`](crate::buffer::LogBuffer::writer) and
//! [`LogBuffer::reader`](crate::buffer::LogBuffer::reader).

mod reader;
mod writer;

pub use reader::Reader;
pub use writer::Writer;
