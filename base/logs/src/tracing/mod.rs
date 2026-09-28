//! All of the `tracing`-crate integration

mod collect;
#[cfg(feature = "console")]
mod console;
mod convert;
mod layer;

#[cfg(feature = "c_api")]
pub(crate) use collect::FieldValue;
pub use layer::BufferLayer;
pub(crate) use layer::{HEX_ID_FIELDS, MESSAGE_FIELD};
