#![allow(clippy::module_name_repetitions)]

extern crate alloc;

#[allow(unused_imports)]
use alloc::vec;
use alloc::{
    borrow::ToOwned,
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::{convert::TryFrom, fmt, str};

use dusk_capnp::capnp::Error;
use indexmap::IndexMap;
use serde::de::{self, MapAccess, SeqAccess, Visitor, value::MapAccessDeserializer};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use dusk_capnp::dusk_capnp::value;
use rapidhash::fast::SeedableState;

pub fn key_bytes_to_string(bytes: Vec<u8>) -> String {
    bytes
        .iter()
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                (b as char).to_string()
            } else {
                // Show hex escape for non-printable
                format!("\\x{:02x}", b)
            }
        })
        .collect::<String>()
}

pub fn key_string_to_bytes(s: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some('x') = chars.peek()
        {
            chars.next(); // consume 'x'
            let hi = chars.next();
            let lo = chars.next();
            if let (Some(h), Some(l)) = (hi, lo)
                && let Ok(val) = u8::from_str_radix(&format!("{}{}", h, l), 16)
            {
                bytes.push(val);
                continue;
            }
            // malformed escape → push raw bytes
            bytes.extend_from_slice(b"\\x");
            if let Some(h) = hi {
                bytes.push(h as u8)
            }
            if let Some(l) = lo {
                bytes.push(l as u8)
            }
            continue;
        }
        bytes.push(c as u8);
    }

    bytes
}

pub type FieldsMap = IndexMap<Vec<u8>, Value, SeedableState<'static>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub type_id: u64,
    pub fields: FieldsMap,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Uint(u64),
    Text(String),
    String(String),
    Bytes(Vec<u8>),
    Bool(bool),
    Record(Record),
    List(Vec<Value>),
}

impl Record {
    #[must_use]
    pub fn with_fields<I>(type_id: u64, fields: I) -> Self
    where
        I: IntoIterator<Item = (Vec<u8>, Value)>,
    {
        let mut fields_map: FieldsMap = IndexMap::with_hasher(SeedableState::default());
        for (key, value) in fields {
            fields_map.insert(key, value);
        }
        Self {
            type_id,
            fields: fields_map,
        }
    }
}

struct FieldsMapSerializer<'a> {
    fields: &'a FieldsMap,
}

impl Serialize for FieldsMapSerializer<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut inner = serializer.serialize_map(Some(self.fields.len()))?;
        for (key, value) in self.fields.iter() {
            inner.serialize_entry(&key_bytes_to_string(key.to_vec()), value)?;
        }
        inner.end()
    }
}

#[allow(exported_private_dependencies)]
impl Serialize for Record {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut outer = serializer.serialize_map(Some(1))?;
        let type_id = format!("0x{:016x}", self.type_id);
        outer.serialize_entry(
            &type_id,
            &FieldsMapSerializer {
                fields: &self.fields,
            },
        )?;
        outer.end()
    }
}

struct FieldsMapDeserializer {
    fields: FieldsMap,
}

struct FieldsMapVisitor;

impl<'de> Visitor<'de> for FieldsMapVisitor {
    type Value = FieldsMapDeserializer;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a fields map whose keys are [u8]")
    }

    fn visit_map<M>(self, mut fields: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut fields_map: FieldsMap = IndexMap::with_hasher(SeedableState::default());
        while let Some((key, value)) = fields.next_entry::<String, Value>()? {
            let decoded_key = key_string_to_bytes(&key);
            fields_map.insert(decoded_key, value);
        }
        Ok(FieldsMapDeserializer { fields: fields_map })
    }
}

impl<'de> Deserialize<'de> for FieldsMapDeserializer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(FieldsMapVisitor)
    }
}

struct RecordVisitor;

impl<'de> Visitor<'de> for RecordVisitor {
    type Value = Record;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an object mapping a hex-encoded type id to its fields map")
    }

    fn visit_map<M>(self, mut fields: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut type_id: Option<u64> = None;
        let mut fields_map: Option<FieldsMap> = None;

        while let Some((key, value)) = fields.next_entry::<String, FieldsMapDeserializer>()? {
            if type_id.is_some() {
                return Err(de::Error::custom(
                    "Fields JSON must contain exactly one type id entry",
                ));
            }
            let trimmed = key
                .strip_prefix("0x")
                .or_else(|| key.strip_prefix("0X"))
                .ok_or_else(|| de::Error::custom(format!("type id must start with 0x: {key}")))?;
            let decoded_type = u64::from_str_radix(trimmed, 16)
                .map_err(|err| de::Error::custom(err.to_string()))?;
            type_id = Some(decoded_type);
            fields_map = Some(value.fields);
        }

        let type_id = type_id.ok_or_else(|| de::Error::custom("Fields JSON missing type id"))?;
        let fields = fields_map.unwrap_or_else(|| IndexMap::with_hasher(SeedableState::default()));

        Ok(Record { type_id, fields })
    }
}

#[allow(exported_private_dependencies)]
impl<'de> Deserialize<'de> for Record {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(RecordVisitor)
    }
}

#[allow(exported_private_dependencies)]
impl Serialize for Value {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Value::Null => serializer.serialize_unit(),
            Value::Uint(v) => serializer.serialize_u64(*v),
            Value::Text(string) | Value::String(string) => {
                serializer.serialize_str(string.as_str())
            }
            Value::Bytes(bytes) => serializer.serialize_bytes(bytes.as_slice()),
            Value::Bool(b) => serializer.serialize_bool(*b),
            Value::Record(record) => record.serialize(serializer),
            Value::List(values) => {
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    seq.serialize_element(value)?;
                }
                seq.end()
            }
        }
    }
}

#[allow(exported_private_dependencies)]
impl<'de> Deserialize<'de> for Value {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ValueVisitor;

        impl<'de> Visitor<'de> for ValueVisitor {
            type Value = Value;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("data compatible with Dusk.Value capnp struct")
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Value::Null)
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Value::Null)
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                Value::deserialize(deserializer)
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Value::Bool(value))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Value::Uint(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                // Converts to positive integer if it's negative.
                Ok(Value::Uint(value as u64))
            }

            fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Err(E::custom("floating point numbers are not supported"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Value::String(value.to_owned()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Value::String(value))
            }

            fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Value::Bytes(value.to_vec()))
            }

            fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Value::Bytes(value))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element()? {
                    values.push(value);
                }
                Ok(Value::List(values))
            }

            fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let record = Record::deserialize(MapAccessDeserializer::new(map))?;
                Ok(Value::Record(record))
            }
        }

        deserializer.deserialize_any(ValueVisitor)
    }
}

impl Value {
    /// Decode a `Value` from its Cap'n Proto representation.
    pub fn from_reader(reader: value::Reader<'_>) -> Result<Self, Error> {
        use value::Which;

        match reader.which()? {
            Which::Null(()) => Ok(Value::Null),
            Which::Uint(v) => Ok(Value::Uint(v)),
            Which::Text(text_reader) => {
                let text = text_reader?
                    .to_str()
                    .map_err(|err| Error::failed(err.to_string()))?
                    .to_owned();
                Ok(Value::Text(text))
            }
            Which::String(text_reader) => {
                let text = text_reader?
                    .to_str()
                    .map_err(|err| Error::failed(err.to_string()))?
                    .to_owned();
                Ok(Value::String(text))
            }
            Which::Bytes(data_reader) => Ok(Value::Bytes(data_reader?.to_vec())),
            Which::Bool(b) => Ok(Value::Bool(b)),
            Which::Record(fields_reader) => {
                let fields_reader = fields_reader?;
                let type_id = fields_reader.get_type_id();
                let fields_reader = fields_reader.get_fields()?;
                let mut fields = IndexMap::with_capacity_and_hasher(
                    fields_reader.len() as usize,
                    SeedableState::default(),
                );
                for entry_reader in fields_reader.iter() {
                    let key = entry_reader.get_key()?.to_vec();
                    let value = Value::from_reader(entry_reader.get_value()?)?;
                    fields.insert(key, value);
                }
                Ok(Value::Record(Record { type_id, fields }))
            }
            Which::List(list_reader) => {
                let list_reader = list_reader?;
                let mut values = Vec::with_capacity(list_reader.len() as usize);
                for value_reader in list_reader.iter() {
                    values.push(Value::from_reader(value_reader)?);
                }
                Ok(Value::List(values))
            }
        }
    }

    /// Encode this `Value` into a Cap'n Proto builder.
    pub fn write_to_builder<'a>(&self, mut builder: value::Builder<'a>) -> Result<(), Error> {
        match self {
            Value::Null => {
                builder.set_null(());
            }
            Value::Uint(v) => {
                builder.set_uint(*v);
            }
            Value::Text(text) => {
                builder.set_text(text.as_str());
            }
            Value::String(string) => {
                builder.set_string(string.as_str());
            }
            Value::Bytes(bytes) => {
                builder.set_bytes(bytes.as_slice());
            }
            Value::Bool(b) => {
                builder.set_bool(*b);
            }
            Value::Record(record) => {
                let mut record_builder = builder.reborrow().init_record();
                record_builder.set_type_id(record.type_id);
                let entry_len = record.fields.len();
                let mut fields_builder = record_builder.reborrow().init_fields(entry_len as u32);
                for (index, (key, value)) in record.fields.iter().enumerate() {
                    let mut field_builder = fields_builder.reborrow().get(index as u32);
                    field_builder.set_key(key);
                    let value_builder = field_builder.reborrow().init_value();
                    value.write_to_builder(value_builder)?;
                }
            }
            Value::List(values) => {
                let mut list_builder = builder.reborrow().init_list(values.len() as u32);
                for (index, value) in values.iter().enumerate() {
                    let value_builder = list_builder.reborrow().get(index as u32);
                    value.write_to_builder(value_builder)?;
                }
            }
        }
        Ok(())
    }

    /// Deserialize a `Value` from JSON using Serde.
    pub fn from_json_str(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Serialize this `Value` to JSON using Serde.
    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl<'a> TryFrom<value::Reader<'a>> for Value {
    type Error = Error;

    fn try_from(reader: value::Reader<'a>) -> Result<Self, Self::Error> {
        Value::from_reader(reader)
    }
}
