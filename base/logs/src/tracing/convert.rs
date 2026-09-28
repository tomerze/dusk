//! Takes spans or logs from the tracing crate and builds capnp messages of Logs.Signal type

use super::MESSAGE_FIELD;
use super::collect::FieldValue;
use crate::common_capnp::{any_value, key_value};
use crate::log_record_capnp::log_record;
use crate::logs_capnp::signal;
use crate::span_capnp::span;
use alloc::format;
use alloc::vec::Vec;
use capnp::message::{Builder, HeapAllocator};
use dusk_program::embassy_time::Instant;

pub(crate) fn build_log_record(
    event_fields: &[(&str, FieldValue)],
    span_id: Option<u64>,
    trace_id: Option<u64>,
) -> Builder<HeapAllocator> {
    let body = event_fields
        .iter()
        .find(|(name, _)| *name == MESSAGE_FIELD)
        .map(|(_, value)| value);

    let attributes: Vec<(&str, &FieldValue)> = event_fields
        .iter()
        .filter(|(name, _)| *name != MESSAGE_FIELD)
        .map(|(name, value)| (*name, value))
        .collect();

    let mut message = Builder::new_default();
    {
        let mut log_record = message.init_root::<signal::Builder>().init_log_record();
        // Stamp embassy-relative milliseconds now (the writer no longer can -
        // the log record is wrapped in a Signal); enrichment converts it to Unix
        // time on the way out.
        log_record.set_time_unix_nano(Instant::now().as_millis());

        if let Some(span_id) = span_id {
            log_record.set_span_id(&span_id.to_be_bytes());
        }
        if let Some(trace_id) = trace_id {
            // The u64 id right-aligned in a 16-byte OTLP trace id.
            let mut trace_id_bytes = [0u8; 16];
            trace_id_bytes[8..].copy_from_slice(&trace_id.to_be_bytes());
            log_record.set_trace_id(&trace_id_bytes);
        }
        log_record
            .reborrow()
            .init_attributes(attributes.len() as u32);
        if let Some(value) = body {
            let mut body_value = log_record.reborrow().init_body();
            match value {
                FieldValue::Text(text) => body_value.set_string_value(text.as_str()),
                FieldValue::Int(integer) => body_value.set_string_value(format!("{integer}")),
                FieldValue::Double(double) => body_value.set_string_value(format!("{double:?}")),
                FieldValue::Bool(boolean) => body_value.set_string_value(format!("{boolean}")),
            }
        }

        let mut filler = LogRecordFiller {
            log_record,
            next_attribute: 0,
        };
        for &(name, value) in &attributes {
            filler.put(name, value);
        }
    }
    message
}

struct LogRecordFiller<'a> {
    log_record: log_record::Builder<'a>,
    next_attribute: u32,
}

impl LogRecordFiller<'_> {
    fn attribute(&mut self, name: &str) -> key_value::Builder<'_> {
        let index = self.next_attribute;
        self.next_attribute += 1;
        let mut attribute = self
            .log_record
            .reborrow()
            .get_attributes()
            .expect("attributes list was initialized before filling")
            .get(index);
        attribute.set_key(name);
        attribute
    }

    fn put(&mut self, name: &str, value: &FieldValue) {
        set_any_value(self.attribute(name).init_value(), value);
    }
}

pub(crate) fn set_any_value(mut builder: any_value::Builder, value: &FieldValue) {
    match value {
        FieldValue::Int(integer) => builder.set_int_value(*integer),
        FieldValue::Double(double) => builder.set_double_value(*double),
        FieldValue::Bool(boolean) => builder.set_bool_value(*boolean),
        FieldValue::Text(text) => builder.set_string_value(text.as_str()),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_span(
    span_id: u64,
    trace_id: Option<u64>,
    parent_span_id: Option<u64>,
    name: &str,
    root: bool,
    start_milliseconds: u64,
    end_milliseconds: Option<u64>,
    attributes: &[(&'static str, FieldValue)],
) -> Builder<HeapAllocator> {
    let mut message = Builder::new_default();
    {
        let mut builder = message.init_root::<signal::Builder>().init_span();
        if let Some(trace_id) = trace_id {
            // The u64 id right-aligned in a 16-byte OTLP trace id.
            let mut trace_id_bytes = [0u8; 16];
            trace_id_bytes[8..].copy_from_slice(&trace_id.to_be_bytes());
            builder.set_trace_id(&trace_id_bytes);
        }
        builder.set_span_id(&span_id.to_be_bytes());
        if let Some(parent_span_id) = parent_span_id {
            builder.set_parent_span_id(&parent_span_id.to_be_bytes());
        }
        builder.set_name(name);
        builder.set_kind(if root {
            span::SpanKind::Server
        } else {
            span::SpanKind::Internal
        });
        builder.set_start_time_unix_nano(start_milliseconds);
        // An absent end stays at the capnp default 0: the span is still open,
        // and the read path treats 0 as "no end yet".
        if let Some(end_milliseconds) = end_milliseconds {
            builder.set_end_time_unix_nano(end_milliseconds);
        }

        let mut attribute_list = builder.init_attributes(attributes.len() as u32);
        for (index, (name, value)) in attributes.iter().enumerate() {
            let mut entry = attribute_list.reborrow().get(index as u32);
            entry.set_key(*name);
            set_any_value(entry.init_value(), value);
        }
    }
    message
}
