//! capnp wire form → owned OTLP protobuf types — the `otlp://` stream only.
//! The dusk log record is OTLP-shaped, so the conversion is field-for-field;
//! this stream needs the real OTLP types because it speaks OTLP/gRPC.

use crate::log_record_capnp::{any_value, key_value, log_record};
use dusk_program::anyhow::Result;
use opentelemetry_proto::tonic::common::v1 as otlp_common;
use opentelemetry_proto::tonic::logs::v1 as otlp_logs;
use std::vec::Vec;

/// One streamed batch → owned OTLP records. A record that fails to convert is
/// skipped with a warning — one bad record must not fail the whole batch.
pub(super) fn records(
    entries: capnp::struct_list::Reader<log_record::Owned>,
) -> Vec<otlp_logs::LogRecord> {
    let mut converted = Vec::new();
    for entry in entries {
        match record(entry) {
            Ok(record) => converted.push(record),
            Err(error) => tracing::warn!(%error, "skipping an unconvertible log record"),
        }
    }
    converted
}

fn record(reader: log_record::Reader) -> Result<otlp_logs::LogRecord> {
    Ok(otlp_logs::LogRecord {
        time_unix_nano: reader.get_time_unix_nano(),
        observed_time_unix_nano: reader.get_observed_time_unix_nano(),
        // Unknown ordinals (a newer node) pass through by number — both sides
        // are the OTLP enum.
        severity_number: match reader.get_severity_number() {
            Ok(severity) => severity as i32,
            Err(capnp::NotInSchema(number)) => number as i32,
        },
        severity_text: reader.get_severity_text()?.to_string()?,
        body: if reader.has_body() {
            Some(value(reader.get_body()?)?)
        } else {
            None
        },
        attributes: attributes(reader.get_attributes()?)?,
        dropped_attributes_count: reader.get_dropped_attributes_count(),
        flags: reader.get_flags(),
        trace_id: reader.get_trace_id()?.to_vec(),
        span_id: reader.get_span_id()?.to_vec(),
        event_name: reader.get_event_name()?.to_string()?,
    })
}

fn value(reader: any_value::Reader) -> Result<otlp_common::AnyValue> {
    use otlp_common::any_value::Value;
    let value = match reader.which()? {
        any_value::Which::StringValue(text) => Value::StringValue(text?.to_string()?),
        any_value::Which::BoolValue(boolean) => Value::BoolValue(boolean),
        any_value::Which::IntValue(integer) => Value::IntValue(integer),
        any_value::Which::DoubleValue(double) => Value::DoubleValue(double),
        any_value::Which::BytesValue(bytes) => Value::BytesValue(bytes?.to_vec()),
        any_value::Which::ArrayValue(array) => Value::ArrayValue(otlp_common::ArrayValue {
            values: array?
                .get_values()?
                .iter()
                .map(value)
                .collect::<Result<_>>()?,
        }),
        any_value::Which::KvlistValue(list) => Value::KvlistValue(otlp_common::KeyValueList {
            values: attributes(list?.get_values()?)?,
        }),
    };
    Ok(otlp_common::AnyValue { value: Some(value) })
}

fn attributes(
    list: capnp::struct_list::Reader<key_value::Owned>,
) -> Result<Vec<otlp_common::KeyValue>> {
    list.iter()
        .map(|attribute| {
            Ok(otlp_common::KeyValue {
                key: attribute.get_key()?.to_string()?,
                value: if attribute.has_value() {
                    Some(value(attribute.get_value()?)?)
                } else {
                    None
                },
            })
        })
        .collect()
}
