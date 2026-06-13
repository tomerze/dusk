//! Turning a stored record into an enriched OTLP `LogRecord`.

use crate::layer::HEX_ID_FIELDS;
use crate::log_record_capnp::{SeverityNumber, any_value, log_record};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use capnp::message::{self, Builder, HeapAllocator, ReaderOptions};
use capnp::serialize::OwnedSegments;
use capnp::serialize_packed;
use dusk_program::embassy_time::Instant;
use tracing::Level;

/// Cap on a record's unpacked size (in 8-byte words): capnp allocates the
/// declared segment sizes up front, and corrupt bytes could otherwise demand the
/// stock 64 MiB in one zeroed allocation. A record above 8 MiB is skipped.
const UNPACK_TRAVERSAL_LIMIT_WORDS: usize = (8 << 20) / 8;

/// Unpacks a packed record into a fresh message and fills in the severity and
/// timestamps for `level`.
pub(super) fn unpack_and_enrich(
    packed: &[u8],
    level: Level,
    offset_from_unix_time_ms: u64,
) -> capnp::Result<Builder<HeapAllocator>> {
    let mut options = ReaderOptions::new();
    options.traversal_limit_in_words(Some(UNPACK_TRAVERSAL_LIMIT_WORDS));
    let message = serialize_packed::read_message(packed, options)?;
    enrich(&message, level, offset_from_unix_time_ms)
}

/// Copies an unpacked record into a fresh message, fills in the severity for
/// `level`, and turns its embassy-relative timestamp into real Unix time.
///
/// The stored record carries embassy-relative milliseconds (time since node
/// startup) in `time_unix_nano`; adding `offset_from_unix_time_ms` yields real
/// Unix milliseconds, which the OTLP field then holds as nanoseconds.
/// `observed_time` is set to now (when the record is read), likewise converted.
fn enrich(
    message: &message::Reader<OwnedSegments>,
    level: Level,
    offset_from_unix_time_ms: u64,
) -> capnp::Result<Builder<HeapAllocator>> {
    let stored_record = message.get_root::<log_record::Reader>()?;

    // Saturating: corrupt records or an absurd settime offset clamp, not panic.
    let logged_unix_ms = stored_record
        .get_time_unix_nano()
        .saturating_add(offset_from_unix_time_ms);
    let observed_unix_ms = Instant::now()
        .as_millis()
        .saturating_add(offset_from_unix_time_ms);

    let mut enriched = Builder::new_default();
    enriched.set_root::<log_record::Owned>(stored_record)?;
    {
        let mut record = enriched.get_root::<log_record::Builder>()?;
        record.set_severity_number(severity(level));
        record.set_severity_text(level.as_str());
        record.set_time_unix_nano(logged_unix_ms.saturating_mul(1_000_000));
        record.set_observed_time_unix_nano(observed_unix_ms.saturating_mul(1_000_000));
    }
    Ok(enriched)
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
