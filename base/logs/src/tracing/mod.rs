//! All of the `tracing`-crate integration

mod collect;
#[cfg(feature = "console")]
mod console;
mod convert;
mod layer;

pub use layer::BufferLayer;
pub(crate) use layer::{HEX_ID_FIELDS, MESSAGE_FIELD};
