//! [`BufferLayer`]: the `tracing` layer that captures events into a
//! [`LogBuffer`](crate::LogBuffer). Each event becomes a packed [`log_record`]:
//! message → OTLP `body`; event fields, span-scope fields (`task_id`, `pid`, …)
//! and source location → `attributes`. Timestamping and severity are not the
//! layer's job — the writer stamps, enrichment fills severity on the way out.
//!
//! no_std: live spans' fields and parentage are tracked in the layer's own map
//! (the std-only subscriber registry is not used), so the layer composes onto
//! any base subscriber. Lock-free into the buffer from any thread — each event
//! packs through its own short-lived [`Writer`](crate::Writer); span bookkeeping
//! takes a brief critical section.

use crate::buffer::{LEVELS, LogBuffer, level_index};
use crate::log_record_capnp::{key_value, log_record};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use capnp::message::Builder;
use core::cell::RefCell;
use dusk_program::embassy_sync::blocking_mutex::Mutex;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use portable_atomic_util::Arc;
use tracing::field::{Field, Visit};
use tracing::span;
use tracing_subscriber::layer::Context;

/// The event's format-string field; it becomes the OTLP `body`.
const MESSAGE_FIELD: &str = "message";

/// Install the global subscriber: the buffer capture layer plus console output
/// at INFO. A no-op if a subscriber is already installed (e.g. a test harness
/// that set its own) — the buffer then simply receives no events.
#[cfg(feature = "console")]
pub(crate) fn bootstrap(buffer: LogBuffer) {
    use tracing_subscriber::layer::{Layer, SubscriberExt};
    use tracing_subscriber::util::SubscriberInitExt;
    let _ = tracing_subscriber::registry()
        .with(BufferLayer::new(buffer))
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_timer(tracing_subscriber::fmt::time::ChronoLocal::rfc_3339())
                .with_filter(tracing_subscriber::filter::LevelFilter::INFO),
        )
        .try_init();
}

/// Parent-chain walk cap — a reused span id could otherwise form a cycle.
const MAX_SPAN_DEPTH: usize = 32;

pub struct BufferLayer {
    buffer: LogBuffer,
    /// Whether `LEVELS[index]` routes to a lane.
    routed: [bool; 5],
    /// Live spans' fields and parentage, keyed by span id.
    spans: Mutex<CriticalSectionRawMutex, RefCell<BTreeMap<u64, SpanRecord>>>,
}

struct SpanRecord {
    parent: Option<u64>,
    fields: Arc<Vec<(&'static str, FieldValue)>>,
}

impl BufferLayer {
    pub fn new(buffer: LogBuffer) -> Self {
        let routed = core::array::from_fn(|index| buffer.routes(LEVELS[index]));
        BufferLayer {
            buffer,
            routed,
            spans: Mutex::new(RefCell::new(BTreeMap::new())),
        }
    }

    /// The field sets of `leaf` and its ancestors, root-first. Only Arc pointers
    /// are cloned under the lock.
    fn scope_fields(&self, leaf: Option<u64>) -> Vec<Arc<Vec<(&'static str, FieldValue)>>> {
        if leaf.is_none() {
            return Vec::new();
        }
        self.spans.lock(|spans| {
            let spans = spans.borrow();
            let mut chain = Vec::new();
            let mut current = leaf;
            while let Some(id) = current {
                if chain.len() >= MAX_SPAN_DEPTH {
                    break;
                }
                let Some(record) = spans.get(&id) else { break };
                chain.push(record.fields.clone());
                current = record.parent;
            }
            chain.reverse();
            chain
        })
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::layer::Layer<S> for BufferLayer {
    fn on_new_span(
        &self,
        attributes: &span::Attributes<'_>,
        id: &span::Id,
        context: Context<'_, S>,
    ) {
        let mut collector = FieldCollector::default();
        attributes.record(&mut collector);
        let parent = if let Some(parent) = attributes.parent() {
            Some(parent.into_u64())
        } else if attributes.is_contextual() {
            context
                .current_span()
                .id()
                .map(|current| current.into_u64())
        } else {
            None
        };
        let record = SpanRecord {
            parent,
            fields: Arc::new(collector.fields),
        };
        self.spans.lock(|spans| {
            spans.borrow_mut().insert(id.into_u64(), record);
        });
    }

    fn on_record(&self, id: &span::Id, values: &span::Record<'_>, _context: Context<'_, S>) {
        // Collect outside the lock — visiting runs caller formatting code.
        let existing = self.spans.lock(|spans| {
            spans
                .borrow()
                .get(&id.into_u64())
                .map(|record| (*record.fields).clone())
        });
        let Some(fields) = existing else { return };
        let mut collector = FieldCollector { fields };
        values.record(&mut collector);
        self.spans.lock(|spans| {
            if let Some(record) = spans.borrow_mut().get_mut(&id.into_u64()) {
                record.fields = Arc::new(collector.fields);
            }
        });
    }

    fn on_close(&self, id: span::Id, _context: Context<'_, S>) {
        self.spans.lock(|spans| {
            spans.borrow_mut().remove(&id.into_u64());
        });
    }

    fn on_event(&self, event: &tracing::Event<'_>, context: Context<'_, S>) {
        let metadata = event.metadata();
        if !self.routed[level_index(*metadata.level())] {
            return;
        }

        let mut counter = AttributeCounter::default();
        event.record(&mut counter);

        let leaf = if let Some(parent) = event.parent() {
            Some(parent.into_u64())
        } else if event.is_contextual() {
            context
                .current_span()
                .id()
                .map(|current| current.into_u64())
        } else {
            None
        };
        let scope = self.scope_fields(leaf);
        let span_attributes: u32 = scope.iter().map(|fields| fields.len() as u32).sum();

        let location_attributes = 1 // target
            + metadata.file().is_some() as u32
            + metadata.line().is_some() as u32
            + metadata.module_path().is_some() as u32;

        let mut message = Builder::new_default();
        {
            let mut record = message.init_root::<log_record::Builder>();
            record
                .reborrow()
                .init_attributes(counter.attributes + span_attributes + location_attributes);

            let mut filler = RecordFiller {
                record,
                next_attribute: 0,
            };
            event.record(&mut filler);
            // Root-first, so an inner span's same-named field lands last.
            for fields in &scope {
                for (name, value) in fields.iter() {
                    filler.put(name, value);
                }
            }
            filler.put_text("target", metadata.target());
            if let Some(file) = metadata.file() {
                filler.put_text("code.filepath", file);
            }
            if let Some(line) = metadata.line() {
                filler.put_int("code.lineno", line as i64);
            }
            if let Some(module_path) = metadata.module_path() {
                filler.put_text("code.namespace", module_path);
            }
        }

        let result = self.buffer.writer().write(*metadata.level(), &mut message);
        // Can't log from inside on_event (it would recurse into this layer);
        // write() counted the failure in drop_counts().write_failures.
        debug_assert!(result.is_ok(), "writing a log record to the buffer failed");
    }
}

#[derive(Clone)]
enum FieldValue {
    Int(i64),
    Double(f64),
    Bool(bool),
    Text(String),
}

/// Collects span fields into owned values; a re-recorded name replaces its
/// earlier value.
#[derive(Default)]
struct FieldCollector {
    fields: Vec<(&'static str, FieldValue)>,
}

impl FieldCollector {
    fn put(&mut self, field: &Field, value: FieldValue) {
        match self
            .fields
            .iter_mut()
            .find(|(name, _)| *name == field.name())
        {
            Some((_, existing)) => *existing = value,
            None => self.fields.push((field.name(), value)),
        }
    }
}

impl Visit for FieldCollector {
    fn record_debug(&mut self, field: &Field, value: &dyn core::fmt::Debug) {
        self.put(field, FieldValue::Text(format!("{value:?}")));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.put(field, FieldValue::Text(value.to_string()));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.put(field, FieldValue::Int(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        match i64::try_from(value) {
            Ok(value) => self.put(field, FieldValue::Int(value)),
            Err(_) => self.put(field, FieldValue::Text(value.to_string())),
        }
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.put(field, FieldValue::Double(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.put(field, FieldValue::Bool(value));
    }
}

/// Counts the fields that will become attributes — the capnp list must be sized
/// up front.
#[derive(Default)]
struct AttributeCounter {
    attributes: u32,
}

impl Visit for AttributeCounter {
    fn record_debug(&mut self, field: &Field, _value: &dyn core::fmt::Debug) {
        if field.name() != MESSAGE_FIELD {
            self.attributes += 1;
        }
    }
}

/// Writes the message into `body` and every other field into the pre-sized
/// `attributes` list.
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

    fn put_text(&mut self, name: &str, value: &str) {
        self.attribute(name).init_value().set_string_value(value);
    }

    fn put_int(&mut self, name: &str, value: i64) {
        self.attribute(name).init_value().set_int_value(value);
    }
}

impl Visit for RecordFiller<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn core::fmt::Debug) {
        let text = format!("{value:?}");
        if field.name() == MESSAGE_FIELD {
            self.record
                .reborrow()
                .init_body()
                .set_string_value(text.as_str());
        } else {
            self.put_text(field.name(), text.as_str());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == MESSAGE_FIELD {
            self.record.reborrow().init_body().set_string_value(value);
        } else {
            self.put_text(field.name(), value);
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name() == MESSAGE_FIELD {
            self.record_debug(field, &value);
        } else {
            self.put_int(field.name(), value);
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        match i64::try_from(value) {
            Ok(value) if field.name() != MESSAGE_FIELD => self.put_int(field.name(), value),
            _ => self.record_debug(field, &value),
        }
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name() == MESSAGE_FIELD {
            self.record_debug(field, &value);
        } else {
            self.attribute(field.name())
                .init_value()
                .set_double_value(value);
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == MESSAGE_FIELD {
            self.record_debug(field, &value);
        } else {
            self.attribute(field.name())
                .init_value()
                .set_bool_value(value);
        }
    }
}
