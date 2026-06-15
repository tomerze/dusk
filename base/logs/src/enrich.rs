//! Turning a stored `Signal` into an enriched one: a log record gets its
//! severity and real timestamps; a span gets its real start/end. Both keep the
//! `Signal` wrapper so the read path stays signal-shaped.

use crate::common_capnp::{any_value, key_value};
use crate::log_record_capnp::{SeverityNumber, log_record};
use crate::logs_capnp::signal;
use crate::span_capnp::span;
use crate::tracing::HEX_ID_FIELDS;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use capnp::message::{self, Builder, HeapAllocator, ReaderOptions};
use capnp::serialize::OwnedSegments;
use capnp::serialize_packed;
use dusk_program::embassy_time::Instant;
use tracing::Level;

/// Cap on a signal's unpacked size (in 8-byte words): capnp allocates the
/// declared segment sizes up front, and corrupt bytes could otherwise demand the
/// stock 64 MiB in one zeroed allocation. A signal above 8 MiB is skipped.
const UNPACK_TRAVERSAL_LIMIT_WORDS: usize = (8 << 20) / 8;

/// Unpacks a packed `Signal` into a fresh message and enriches it for `level`.
pub(crate) fn unpack_and_enrich(
    packed: &[u8],
    level: Level,
    offset_from_unix_time_ms: u64,
    global_sequence: u64,
) -> capnp::Result<Builder<HeapAllocator>> {
    let mut options = ReaderOptions::new();
    options.traversal_limit_in_words(Some(UNPACK_TRAVERSAL_LIMIT_WORDS));
    let message = serialize_packed::read_message(packed, options)?;
    enrich(&message, level, offset_from_unix_time_ms, global_sequence)
}

fn enrich(
    message: &message::Reader<OwnedSegments>,
    level: Level,
    offset_from_unix_time_ms: u64,
    global_sequence: u64,
) -> capnp::Result<Builder<HeapAllocator>> {
    let mut output = Builder::new_default();
    {
        let mut signal = output.init_root::<signal::Builder>();
        signal.set_severity_number(severity(level));
        signal.set_global_sequence(global_sequence);
        match message.get_root::<signal::Reader>()?.which()? {
            signal::Which::LogRecord(log_record) => {
                let enriched = enrich_log_record(log_record?, level, offset_from_unix_time_ms)?;
                signal.set_log_record(enriched.get_root_as_reader::<log_record::Reader>()?)?;
            }
            signal::Which::Span(span) => {
                let enriched = enrich_span(span?, offset_from_unix_time_ms)?;
                signal.set_span(enriched.get_root_as_reader::<span::Reader>()?)?;
            }
        }
    }
    Ok(output)
}

fn enrich_log_record(
    stored: log_record::Reader,
    level: Level,
    offset_from_unix_time_ms: u64,
) -> capnp::Result<Builder<HeapAllocator>> {
    // Saturating: corrupt log records or an absurd settime offset clamp, not panic.
    let logged_unix_ms = stored
        .get_time_unix_nano()
        .saturating_add(offset_from_unix_time_ms);
    let observed_unix_ms = Instant::now()
        .as_millis()
        .saturating_add(offset_from_unix_time_ms);
    let hex_ids = hex_id_attributes(stored.get_attributes()?)?;

    let mut enriched = Builder::new_default();
    enriched.set_root::<log_record::Owned>(stored)?;
    {
        // The buffer stores the log record's severity empty (it rides the `Signal`
        // envelope); fill the OTLP fields back in here, on the way out — both the
        // numeric severity and its text — so a streamed log record is complete.
        let mut log_record = enriched.get_root::<log_record::Builder>()?;
        log_record.set_severity_number(severity(level));
        log_record.set_severity_text(level.as_str());
        log_record.set_time_unix_nano(logged_unix_ms.saturating_mul(1_000_000));
        log_record.set_observed_time_unix_nano(observed_unix_ms.saturating_mul(1_000_000));
        rewrite_hex_ids(log_record.reborrow().get_attributes()?, &hex_ids)?;
    }
    Ok(enriched)
}

/// A span with its embassy-relative start/end turned into real Unix nanoseconds.
/// Spans carry no severity, so `level` does not apply.
fn enrich_span(
    stored: span::Reader,
    offset_from_unix_time_ms: u64,
) -> capnp::Result<Builder<HeapAllocator>> {
    let start_unix_ms = stored
        .get_start_time_unix_nano()
        .saturating_add(offset_from_unix_time_ms);
    let end_unix_ms = stored
        .get_end_time_unix_nano()
        .saturating_add(offset_from_unix_time_ms);
    let hex_ids = hex_id_attributes(stored.get_attributes()?)?;

    let mut enriched = Builder::new_default();
    enriched.set_root::<span::Owned>(stored)?;
    {
        let mut span = enriched.get_root::<span::Builder>()?;
        span.set_start_time_unix_nano(start_unix_ms.saturating_mul(1_000_000));
        span.set_end_time_unix_nano(end_unix_ms.saturating_mul(1_000_000));
        rewrite_hex_ids(span.reborrow().get_attributes()?, &hex_ids)?;
    }
    Ok(enriched)
}

fn hex_id_attributes(
    attributes: capnp::struct_list::Reader<key_value::Owned>,
) -> capnp::Result<Vec<(u32, String)>> {
    let mut hex_ids = Vec::new();
    for (index, attribute) in attributes.iter().enumerate() {
        if !HEX_ID_FIELDS.contains(&attribute.get_key()?.to_str()?) {
            continue;
        }
        let id = match attribute.get_value()?.which()? {
            any_value::Which::IntValue(integer) => integer as u64,
            any_value::Which::StringValue(text) => match text?.to_str()?.parse::<u64>() {
                Ok(id) => id,
                Err(_) => continue,
            },
            _ => continue,
        };
        hex_ids.push((index as u32, format!("{id:x}")));
    }
    Ok(hex_ids)
}

/// Overwrite the listed attribute values with their bare-hex strings.
fn rewrite_hex_ids(
    mut attributes: capnp::struct_list::Builder<key_value::Owned>,
    hex_ids: &[(u32, String)],
) -> capnp::Result<()> {
    for (index, hex) in hex_ids {
        attributes
            .reborrow()
            .get(*index)
            .get_value()?
            .set_string_value(hex.as_str());
    }
    Ok(())
}

/// OTLP severity number for a `tracing` level (the base of each OTLP severity
/// group). The matching severity text is `level.as_str()`.
fn severity(level: Level) -> SeverityNumber {
    match level {
        Level::ERROR => SeverityNumber::Error,
        Level::WARN => SeverityNumber::Warn,
        Level::INFO => SeverityNumber::Info,
        Level::DEBUG => SeverityNumber::Debug,
        _ => SeverityNumber::Trace,
    }
}
