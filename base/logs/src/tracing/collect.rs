use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use tracing::field::{Field, Visit};

#[derive(Clone)]
pub(crate) enum FieldValue {
    Int(i64),
    Double(f64),
    Bool(bool),
    Text(String),
}

#[derive(Default)]
pub(crate) struct FieldCollector {
    pub(crate) fields: Vec<(&'static str, FieldValue)>,
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

    // A JSON number is a double — exact only to 2^53 — so a 64-bit-magnitude
    // integer can't round-trip through one (Kibana). Only 32-bit-range values
    // stay an OTLP int; anything wider becomes a string.
    fn record_i64(&mut self, field: &Field, value: i64) {
        match i32::try_from(value) {
            Ok(_) => self.put(field, FieldValue::Int(value)),
            Err(_) => self.put(field, FieldValue::Text(value.to_string())),
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        match i32::try_from(value) {
            Ok(_) => self.put(field, FieldValue::Int(value as i64)),
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
