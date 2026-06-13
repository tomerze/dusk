//! The client-side log streams: the consumers of the node's streamed records.
//! Each stream is an implementation of the `LogsArgs.Server` capnp interface
//! (`send`/`stop`) — the node pushes records in through [`pipe`](crate::pipe)
//! and the stream writes them wherever it lands. The built-ins live one per
//! submodule; an external author implements the same interface to write their
//! own.
//!
//! The dusk log record (`log_record.capnp`) is itself OTLP-shaped, so most
//! streams need no OTLP library: [`records_json`] renames it field-for-field
//! into OTLP/JSON. Only [`otlp`] — which speaks OTLP/gRPC protobuf — converts to
//! the OTLP types, and it does so privately.
//!
//! Backpressure is the `send` itself: a stream resolves its `send` only once it
//! has written the batch (or has room), which fills the node's flow-control
//! window and parks it. A stream that fails fatally triggers its
//! [`stop`](stop_promise) signal — the node's cue to finish.
//!
//! - [`file`] — `file://<path>`: appends one OTLP/JSON record per line (jsonl).
//! - [`http`] — `http://` / `https://`: one POST per record, OTLP/JSON.
//! - [`otlp`] — `otlp://<host:port>`: OTLP/gRPC `Export` calls, one per batch.
//! - [`viewer`] — the interactive terminal pager (no url).
//! - [`print`] — a plain stdout dump (no url, `--replay-only`).

use std::path::PathBuf;
use std::rc::Rc;
use std::string::String;
use std::vec::Vec;

use crate::log_record_capnp::{any_value, key_value, log_record};
use crate::logs_capnp::logs_args;
use base64::Engine as _;
use capnp::capability::Promise;
use dusk_program::anyhow::{Result, bail};
use dusk_program::dusk_capnp::capnp_rpc;
use tokio::sync::Notify;

pub mod file;
pub mod http;
pub mod otlp;
pub mod print;
pub mod viewer;

pub use file::FileStream;
pub use http::HttpStream;
pub use otlp::OtlpStream;
pub use print::PrintStream;
pub use viewer::ViewerStream;

/// Build the built-in stream for a `logs stream` URL. The viewer has no url; it
/// is constructed directly ([`ViewerStream`]).
pub(crate) fn parse(url: &str) -> Result<logs_args::server::Client> {
    let server: logs_args::server::Client = if let Some(path) = url.strip_prefix("file://") {
        if path.is_empty() {
            bail!("file:// needs a path: {url}");
        }
        capnp_rpc::new_client(FileStream::new(PathBuf::from(path)))
    } else if let Some(authority) = url.strip_prefix("otlp://") {
        if authority.is_empty() {
            bail!("otlp:// needs a host:port: {url}");
        }
        capnp_rpc::new_client(OtlpStream::new(format!("http://{authority}")))
    } else if url.starts_with("http://") || url.starts_with("https://") {
        capnp_rpc::new_client(HttpStream::new(url.to_string()))
    } else {
        bail!("unsupported url: {url} (expected file://, otlp://, http:// or https://)")
    };
    Ok(server)
}

/// Answer the node's `stop` long-poll once `stop` is signalled — the cue to
/// finish the stream. Shared by every built-in: a stream signals it when it
/// fails fatally or (the viewer) when the user quits.
pub(crate) fn stop_promise(stop: &Rc<Notify>) -> Promise<(), capnp::Error> {
    let stop = stop.clone();
    Promise::from_future(async move {
        stop.notified().await;
        Ok(())
    })
}

pub(crate) fn records_json(
    entries: capnp::struct_list::Reader<log_record::Owned>,
) -> Vec<serde_json::Value> {
    let mut converted = Vec::new();
    for entry in entries {
        match record_json(entry) {
            Ok(record) => converted.push(record),
            Err(error) => tracing::warn!(%error, "skipping an unconvertible log record"),
        }
    }
    converted
}

/// One capnp log record → its OTLP/JSON object: camelCase field names, 64-bit
/// times as strings, values in the OTLP `AnyValue` shape. Default-valued fields
/// are omitted, per the OTLP/JSON encoding.
fn record_json(record: log_record::Reader) -> capnp::Result<serde_json::Value> {
    let mut object = serde_json::Map::new();

    let time = record.get_time_unix_nano();
    if time != 0 {
        object.insert("timeUnixNano".into(), time.to_string().into());
    }
    let observed = record.get_observed_time_unix_nano();
    if observed != 0 {
        object.insert("observedTimeUnixNano".into(), observed.to_string().into());
    }
    // Unknown ordinals (a newer node) pass through by number — both sides are
    // the OTLP enum.
    let severity_number = match record.get_severity_number() {
        Ok(severity) => severity as i32,
        Err(capnp::NotInSchema(number)) => number as i32,
    };
    if severity_number != 0 {
        object.insert("severityNumber".into(), severity_number.into());
    }
    let severity_text = record.get_severity_text()?.to_str()?;
    if !severity_text.is_empty() {
        object.insert("severityText".into(), severity_text.into());
    }
    if record.has_body() {
        object.insert("body".into(), any_value_json(record.get_body()?)?);
    }
    let attributes = record.get_attributes()?;
    if !attributes.is_empty() {
        object.insert("attributes".into(), attributes_json(attributes)?);
    }
    let dropped = record.get_dropped_attributes_count();
    if dropped != 0 {
        object.insert("droppedAttributesCount".into(), dropped.into());
    }
    let flags = record.get_flags();
    if flags != 0 {
        object.insert("flags".into(), flags.into());
    }
    let trace_id = record.get_trace_id()?;
    if !trace_id.is_empty() {
        object.insert("traceId".into(), hex(trace_id).into());
    }
    let span_id = record.get_span_id()?;
    if !span_id.is_empty() {
        object.insert("spanId".into(), hex(span_id).into());
    }
    let event_name = record.get_event_name()?.to_str()?;
    if !event_name.is_empty() {
        object.insert("eventName".into(), event_name.into());
    }

    Ok(serde_json::Value::Object(object))
}

/// An OTLP `AnyValue` as its OTLP/JSON union object (`{"stringValue": …}`, …);
/// int64 is a string and bytes are base64, per OTLP/JSON.
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

/// A `List(KeyValue)` as the OTLP/JSON array of `{"key": …, "value": …}`.
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

/// Lowercase hex — the OTLP/JSON encoding for trace and span ids.
fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write as _;
    let mut string = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(string, "{byte:02x}");
    }
    string
}
