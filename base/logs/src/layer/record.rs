//! Capture, stage two: the [collected](super::collect) [`FieldValue`]s — the
//! event's own fields, its span scope, and its source location — become a
//! packed-ready capnp [`log_record`]. The message field becomes the OTLP
//! `body`; everything else becomes a deduplicated attribute.

use super::MESSAGE_FIELD;
use super::collect::FieldValue;
use crate::log_record_capnp::{key_value, log_record};
use alloc::format;
use alloc::vec::Vec;
use capnp::message::{Builder, HeapAllocator};
use portable_atomic_util::Arc;

/// Build one event's capnp log record from its three field sources.
///
/// OTLP requires attribute keys to be unique within a record, but one name can
/// appear on the event, on several spans up the parent chain, and in the
/// location set. The sources are flattened innermost-first and the first value
/// seen for each key is kept, so precedence is event field > leaf span >
/// ancestor span > location. The message field is pulled out as the body
/// instead of becoming an attribute.
pub(crate) fn assemble_record(
    event_fields: &[(&'static str, FieldValue)],
    scope: &[Arc<Vec<(&'static str, FieldValue)>>],
    location: &[(&'static str, FieldValue)],
) -> Builder<HeapAllocator> {
    let body = event_fields
        .iter()
        .find(|(name, _)| *name == MESSAGE_FIELD)
        .map(|(_, value)| value);

    let mut attributes: Vec<(&'static str, &FieldValue)> = Vec::new();
    let candidates = event_fields
        .iter()
        .filter(|(name, _)| *name != MESSAGE_FIELD)
        .map(|(name, value)| (*name, value))
        .chain(
            scope
                .iter()
                .rev()
                .flat_map(|fields| fields.iter())
                .map(|(name, value)| (*name, value)),
        )
        .chain(location.iter().map(|(name, value)| (*name, value)));
    for (name, value) in candidates {
        if !attributes.iter().any(|(seen, _)| *seen == name) {
            attributes.push((name, value));
        }
    }

    let mut message = Builder::new_default();
    {
        let mut record = message.init_root::<log_record::Builder>();
        record.reborrow().init_attributes(attributes.len() as u32);
        if let Some(value) = body {
            let mut body_value = record.reborrow().init_body();
            match value {
                FieldValue::Text(text) => body_value.set_string_value(text.as_str()),
                FieldValue::Int(integer) => body_value.set_string_value(format!("{integer}")),
                FieldValue::Double(double) => body_value.set_string_value(format!("{double:?}")),
                FieldValue::Bool(boolean) => body_value.set_string_value(format!("{boolean}")),
            }
        }

        let mut filler = RecordFiller {
            record,
            next_attribute: 0,
        };
        for &(name, value) in &attributes {
            filler.put(name, value);
        }
    }
    message
}

/// Writes each `(name, value)` into the pre-sized `attributes` list.
struct RecordFiller<'a> {
    record: log_record::Builder<'a>,
    next_attribute: u32,
}

impl RecordFiller<'_> {
    fn attribute(&mut self, name: &str) -> key_value::Builder<'_> {
        let index = self.next_attribute;
        self.next_attribute += 1;
        let mut attribute = self
            .record
            .reborrow()
            .get_attributes()
            .expect("attributes list was initialized before filling")
            .get(index);
        attribute.set_key(name);
        attribute
    }

    fn put(&mut self, name: &str, value: &FieldValue) {
        let mut builder = self.attribute(name).init_value();
        match value {
            FieldValue::Int(value) => builder.set_int_value(*value),
            FieldValue::Double(value) => builder.set_double_value(*value),
            FieldValue::Bool(value) => builder.set_bool_value(*value),
            FieldValue::Text(value) => builder.set_string_value(value.as_str()),
        }
    }
}
