use crate::common_capnp::{any_value, key_value};
use crate::log_record_capnp::log_record;
use crate::logs_capnp::signal;
use crate::span_capnp::span;
use base64::Engine as _;
use std::string::String;
use std::vec::Vec;

/// The OpenTelemetry-standard unique log-record id: a backend that honours the
/// convention treats records sharing it as duplicates and keeps one. Vendor
/// neutral, and log records only - the convention has no span equivalent.
const LOG_RECORD_UID_ATTRIBUTE: &str = "log.record.uid";

const ELASTICSEARCH_DOCUMENT_ID_ATTRIBUTE: &str = "elasticsearch.document_id";

pub(crate) fn signal_to_json(
    signal: signal::Reader,
    namespace_id: u64,
) -> capnp::Result<serde_json::Value> {
    let unique_id = format!("{:x}-{:x}", namespace_id, signal.get_global_sequence());
    match signal.which()? {
        signal::Which::LogRecord(log_record) => {
            let mut value = log_record_json(log_record?, namespace_id)?;
            add_attribute(&mut value, LOG_RECORD_UID_ATTRIBUTE, &unique_id);
            add_attribute(&mut value, ELASTICSEARCH_DOCUMENT_ID_ATTRIBUTE, &unique_id);
            Ok(value)
        }
        signal::Which::Span(span) => {
            let mut value = span_json(span?, namespace_id)?;
            add_attribute(&mut value, ELASTICSEARCH_DOCUMENT_ID_ATTRIBUTE, &unique_id);
            Ok(value)
        }
    }
}

/// The 16-byte OTLP trace id with `namespace_id` folded into its high 8 bytes.
/// `build_log_record`/`build_span` fill only the low 8 bytes (the task-root id),
/// leaving the high 8 zero; stamping the node's namespace id there makes the
/// trace id globally unique across node runs, so logs and spans from different
/// runs never collide on a trace.
fn mint_trace_id(trace_id: &[u8], namespace_id: u64) -> String {
    let mut bytes = [0u8; 16];
    // Right-align the stored id (its low 8 bytes hold the task-root); a
    // well-formed id is already 16 bytes.
    let copy = trace_id.len().min(16);
    bytes[16 - copy..].copy_from_slice(&trace_id[trace_id.len() - copy..]);
    bytes[..8].copy_from_slice(&namespace_id.to_be_bytes());
    hex(&bytes)
}

/// Append a string-valued attribute to a converted log record or span.
fn add_attribute(value: &mut serde_json::Value, key: &str, attribute_value: &str) {
    let serde_json::Value::Object(object) = value else {
        return;
    };
    let entry = serde_json::json!({
        "key": key,
        "value": { "stringValue": attribute_value },
    });
    match object.get_mut("attributes") {
        Some(serde_json::Value::Array(attributes)) => attributes.push(entry),
        _ => {
            object.insert("attributes".into(), serde_json::json!([entry]));
        }
    }
}

fn log_record_json(
    log_record: log_record::Reader,
    namespace_id: u64,
) -> capnp::Result<serde_json::Value> {
    let mut object = serde_json::Map::new();

    let time = log_record.get_time_unix_nano();
    if time != 0 {
        object.insert("timeUnixNano".into(), time.to_string().into());
    }
    let observed = log_record.get_observed_time_unix_nano();
    if observed != 0 {
        object.insert("observedTimeUnixNano".into(), observed.to_string().into());
    }
    let severity_number = match log_record.get_severity_number() {
        Ok(severity) => severity as i32,
        Err(capnp::NotInSchema(number)) => number as i32,
    };
    if severity_number != 0 {
        object.insert("severityNumber".into(), severity_number.into());
    }
    let severity_text = log_record.get_severity_text()?.to_str()?;
    if !severity_text.is_empty() {
        object.insert("severityText".into(), severity_text.into());
    }
    if log_record.has_body() {
        object.insert("body".into(), any_value_json(log_record.get_body()?)?);
    }
    let attributes = log_record.get_attributes()?;
    if !attributes.is_empty() {
        object.insert("attributes".into(), attributes_json(attributes)?);
    }
    let dropped = log_record.get_dropped_attributes_count();
    if dropped != 0 {
        object.insert("droppedAttributesCount".into(), dropped.into());
    }
    let flags = log_record.get_flags();
    if flags != 0 {
        object.insert("flags".into(), flags.into());
    }
    let trace_id = log_record.get_trace_id()?;
    if !trace_id.is_empty() {
        object.insert(
            "traceId".into(),
            mint_trace_id(trace_id, namespace_id).into(),
        );
    }
    let span_id = log_record.get_span_id()?;
    if !span_id.is_empty() {
        object.insert("spanId".into(), hex(span_id).into());
    }
    let event_name = log_record.get_event_name()?.to_str()?;
    if !event_name.is_empty() {
        object.insert("eventName".into(), event_name.into());
    }

    Ok(serde_json::Value::Object(object))
}

fn span_json(span: span::Reader, namespace_id: u64) -> capnp::Result<serde_json::Value> {
    let mut object = serde_json::Map::new();

    let trace_id = span.get_trace_id()?;
    if !trace_id.is_empty() {
        object.insert(
            "traceId".into(),
            mint_trace_id(trace_id, namespace_id).into(),
        );
    }
    let span_id = span.get_span_id()?;
    if !span_id.is_empty() {
        object.insert("spanId".into(), hex(span_id).into());
    }
    let parent_span_id = span.get_parent_span_id()?;
    if !parent_span_id.is_empty() {
        object.insert("parentSpanId".into(), hex(parent_span_id).into());
    }
    let trace_state = span.get_trace_state()?.to_str()?;
    if !trace_state.is_empty() {
        object.insert("traceState".into(), trace_state.into());
    }
    let flags = span.get_flags();
    if flags != 0 {
        object.insert("flags".into(), flags.into());
    }
    let name = span.get_name()?.to_str()?;
    if !name.is_empty() {
        object.insert("name".into(), name.into());
    }
    // Unknown ordinals (a newer node) pass through by number - both sides are
    // the OTLP enum.
    let kind = match span.get_kind() {
        Ok(kind) => kind as i32,
        Err(capnp::NotInSchema(number)) => number as i32,
    };
    if kind != 0 {
        object.insert("kind".into(), kind.into());
    }
    let start = span.get_start_time_unix_nano();
    if start != 0 {
        object.insert("startTimeUnixNano".into(), start.to_string().into());
    }
    let end = span.get_end_time_unix_nano();
    if end != 0 {
        object.insert("endTimeUnixNano".into(), end.to_string().into());
    }
    let attributes = span.get_attributes()?;
    if !attributes.is_empty() {
        object.insert("attributes".into(), attributes_json(attributes)?);
    }
    let dropped = span.get_dropped_attributes_count();
    if dropped != 0 {
        object.insert("droppedAttributesCount".into(), dropped.into());
    }

    Ok(serde_json::Value::Object(object))
}

fn any_value_json(value: any_value::Reader) -> capnp::Result<serde_json::Value> {
    use any_value::Which;
    let object = match value.which()? {
        Which::StringValue(text) => serde_json::json!({ "stringValue": text?.to_str()? }),
        Which::BoolValue(boolean) => serde_json::json!({ "boolValue": boolean }),
        Which::IntValue(integer) => serde_json::json!({ "intValue": integer.to_string() }),
        Which::DoubleValue(double) => serde_json::json!({ "doubleValue": double }),
        Which::BytesValue(bytes) => serde_json::json!({
            "bytesValue": base64::engine::general_purpose::STANDARD.encode(bytes?),
        }),
        Which::ArrayValue(array) => {
            let values = array?
                .get_values()?
                .iter()
                .map(any_value_json)
                .collect::<capnp::Result<Vec<_>>>()?;
            serde_json::json!({ "arrayValue": { "values": values } })
        }
        Which::KvlistValue(list) => serde_json::json!({
            "kvlistValue": { "values": attributes_json(list?.get_values()?)? },
        }),
    };
    Ok(object)
}

fn attributes_json(
    list: capnp::struct_list::Reader<key_value::Owned>,
) -> capnp::Result<serde_json::Value> {
    let values = list
        .iter()
        .map(|attribute| {
            let value = if attribute.has_value() {
                any_value_json(attribute.get_value()?)?
            } else {
                serde_json::Value::Null
            };
            Ok(serde_json::json!({ "key": attribute.get_key()?.to_str()?, "value": value }))
        })
        .collect::<capnp::Result<Vec<_>>>()?;
    Ok(serde_json::Value::Array(values))
}

fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write as _;
    let mut string = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(string, "{byte:02x}");
    }
    string
}
