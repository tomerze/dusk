use crate::buffer::Reader;
use crate::common_capnp::{any_value, key_value};
use crate::logs_capnp::{BATCH_TYPE_ID, signal};
use crate::streamer::keeps;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use dusk_capnp::dusk_capnp::stream;
use dusk_program::embassy_futures;
use dusk_program::value::{Record, Value};

pub(crate) async fn dump(
    reader: &mut Reader,
    stream: &stream::Client,
    minimum_severity: u16,
    follow: bool,
) -> capnp::Result<()> {
    loop {
        let signal = match reader.try_read() {
            Some(signal) => signal,
            None if !follow && reader.caught_up_to_subscription() => return Ok(()),
            None => reader.read().await,
        };
        if keeps(&signal, minimum_severity)? {
            let value = signal_to_value(signal.get_root_as_reader::<signal::Reader>()?)?;
            let mut request = stream.send_request();
            value.write_to_builder(request.get().init_value())?;
            request.send().await?;
        }
        embassy_futures::yield_now().await;
    }
}

pub(crate) fn signal_to_value(signal: signal::Reader) -> capnp::Result<Value> {
    let severity =
        severity_text(signal.get_severity_number().map(|s| s as u16).unwrap_or(0)).unwrap_or("");
    let mut fields: Vec<(Vec<u8>, Value)> = alloc::vec![
        (
            b"sequence".to_vec(),
            Value::Uint(signal.get_global_sequence())
        ),
        (b"severity".to_vec(), Value::String(severity.to_string())),
    ];
    match signal.which()? {
        signal::Which::LogRecord(log_record) => {
            let log_record = log_record?;
            fields.push((
                b"timeUnixNano".to_vec(),
                Value::Uint(log_record.get_time_unix_nano()),
            ));
            let message = if log_record.has_body() {
                any_value_to_value(log_record.get_body()?)?
            } else {
                Value::String(String::new())
            };
            fields.push((b"message".to_vec(), message));
            if let Some(attributes) = attributes_value(log_record.get_attributes()?)? {
                fields.push((b"attributes".to_vec(), attributes));
            }
        }
        signal::Which::Span(span) => {
            let span = span?;
            fields.push((
                b"name".to_vec(),
                Value::String(span.get_name()?.to_str()?.to_string()),
            ));
            fields.push((
                b"startTimeUnixNano".to_vec(),
                Value::Uint(span.get_start_time_unix_nano()),
            ));
            fields.push((
                b"endTimeUnixNano".to_vec(),
                Value::Uint(span.get_end_time_unix_nano()),
            ));
            if let Some(attributes) = attributes_value(span.get_attributes()?)? {
                fields.push((b"attributes".to_vec(), attributes));
            }
        }
    }
    Ok(Value::Record(Record::with_fields(BATCH_TYPE_ID, fields)))
}

fn attributes_value(
    attributes: capnp::struct_list::Reader<key_value::Owned>,
) -> capnp::Result<Option<Value>> {
    if attributes.is_empty() {
        return Ok(None);
    }
    let mut fields: Vec<(Vec<u8>, Value)> = Vec::with_capacity(attributes.len() as usize);
    for attribute in attributes.iter() {
        let key = attribute.get_key()?.to_str()?.as_bytes().to_vec();
        let value = if attribute.has_value() {
            any_value_to_value(attribute.get_value()?)?
        } else {
            Value::Null
        };
        fields.push((key, value));
    }
    Ok(Some(Value::Record(Record::with_fields(
        BATCH_TYPE_ID,
        fields,
    ))))
}

/// An OTLP `AnyValue` → a Dusk `Value`.
fn any_value_to_value(value: any_value::Reader) -> capnp::Result<Value> {
    use any_value::Which;
    Ok(match value.which()? {
        Which::StringValue(text) => Value::String(text?.to_str()?.to_string()),
        Which::BoolValue(boolean) => Value::Bool(boolean),
        Which::IntValue(integer) if integer >= 0 => Value::Uint(integer as u64),
        Which::IntValue(integer) => Value::String(integer.to_string()),
        Which::DoubleValue(double) => Value::String(double.to_string()),
        Which::BytesValue(bytes) => Value::Bytes(bytes?.to_vec()),
        Which::ArrayValue(array) => {
            let array = array?;
            let mut values = Vec::new();
            for element in array.get_values()?.iter() {
                values.push(any_value_to_value(element)?);
            }
            Value::List(values)
        }
        Which::KvlistValue(kvlist) => {
            let kvlist = kvlist?;
            let mut fields: Vec<(Vec<u8>, Value)> = Vec::new();
            for entry in kvlist.get_values()?.iter() {
                let key = entry.get_key()?.to_str()?.as_bytes().to_vec();
                let entry_value = if entry.has_value() {
                    any_value_to_value(entry.get_value()?)?
                } else {
                    Value::Null
                };
                fields.push((key, entry_value));
            }
            Value::Record(Record::with_fields(BATCH_TYPE_ID, fields))
        }
    })
}

/// The text label for an OTLP severity number, by group. `None` for unspecified
/// (0) or an unknown number. Matches the interactive viewer's labels.
fn severity_text(severity_number: u16) -> Option<&'static str> {
    match severity_number {
        1..=4 => Some("TRACE"),
        5..=8 => Some("DEBUG"),
        9..=12 => Some("INFO"),
        13..=16 => Some("WARN"),
        17..=20 => Some("ERROR"),
        21..=24 => Some("FATAL"),
        _ => None,
    }
}
