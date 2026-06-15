//! The producer and consumer handles over a [`SignalBuffer`](crate::buffer::SignalBuffer):
//! the append-only [`Writer`] and the non-destructive [`Reader`]. Both share their
//! originating buffer's lanes; obtain them via
//! [`SignalBuffer::writer`](crate::buffer::SignalBuffer::writer) and
//! [`SignalBuffer::reader`](crate::buffer::SignalBuffer::reader).

mod reader;
mod writer;

pub use reader::Reader;
pub use writer::Writer;
