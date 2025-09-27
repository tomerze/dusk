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

use capnp::Error;
use hashbrown::HashMap;
use serde::de::{self, value::MapAccessDeserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use dusk_capnp::dusk_capnp::value;
use rapidhash::fast::SeedableState;

pub fn key_bytes_to_string(bytes: Vec<u8>) -> String {
    bytes.iter().map(|&b| {
        if b.is_ascii_graphic() || b == b' ' {
            (b as char).to_string()
        } else {
            // Show hex escape for non-printable
            format!("\\x{:02x}", b)
        }
    }).collect::<String>()
}

pub fn key_string_to_bytes(s: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some('x') = chars.peek() {
                chars.next(); // consume 'x'
                let hi = chars.next();
                let lo = chars.next();
                if let (Some(h), Some(l)) = (hi, lo) {
                    if let Ok(val) = u8::from_str_radix(&format!("{}{}", h, l), 16) {
                        bytes.push(val);
                        continue;
                    }
                }
                // malformed escape → push raw bytes
                bytes.extend_from_slice(b"\\x");
                if let Some(h) = hi { bytes.push(h as u8) }
                if let Some(l) = lo { bytes.push(l as u8) }
                continue;
            }
        }
        bytes.push(c as u8);
    }

    bytes
}

pub type FieldsMap = HashMap<Vec<u8>, Value, SeedableState<'static>>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fields {
    pub type_id: u64,
    pub map: FieldsMap,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Uint(u64),
    Text(String),
    String(String),
    Bytes(Vec<u8>),
    Bool(bool),
    Fields(Fields),
    List(Vec<Value>),
}

impl Fields {
    #[must_use]
    pub fn with_entries<I>(type_id: u64, entries: I) -> Self
    where
        I: IntoIterator<Item = (Vec<u8>, Value)>,
    {
        let mut map = HashMap::with_hasher(SeedableState::default());
        for (key, value) in entries {
            map.insert(key, value);
        }
        Self { type_id, map }
    }
}

struct FieldsEntriesSerializer<'a> {
    map: &'a FieldsMap,
}

impl Serialize for FieldsEntriesSerializer<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut inner = serializer.serialize_map(Some(self.map.len()))?;
        for (key, value) in self.map.iter() {
            inner.serialize_entry(&key_bytes_to_string(key.to_vec()), value)?;
        }
        inner.end()
    }
}

#[allow(exported_private_dependencies)]
impl Serialize for Fields {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut outer = serializer.serialize_map(Some(1))?;
        let type_id = format!("0x{:016x}", self.type_id);
        outer.serialize_entry(&type_id, &FieldsEntriesSerializer { map: &self.map })?;
        outer.end()
    }
}

struct FieldsEntries(HashMap<Vec<u8>, Value, SeedableState<'static>>);

impl FieldsEntries {
    fn into_map(self) -> HashMap<Vec<u8>, Value, SeedableState<'static>> {
        self.0
    }
}

struct FieldsEntriesVisitor;

impl<'de> Visitor<'de> for FieldsEntriesVisitor {
    type Value = FieldsEntries;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a map whose keys are UTF-8 strings or :hex:-prefixed hex data")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut entries = HashMap::with_hasher(SeedableState::default());
        while let Some((key, value)) = map.next_entry::<String, Value>()? {
            let decoded_key = key_string_to_bytes(&key);
            entries.insert(decoded_key, value);
        }
        Ok(FieldsEntries(entries))
    }
}

impl<'de> Deserialize<'de> for FieldsEntries {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(FieldsEntriesVisitor)
    }
}

struct FieldsVisitor;

impl<'de> Visitor<'de> for FieldsVisitor {
    type Value = Fields;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an object mapping a hex-encoded type id to its field entries")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut type_id: Option<u64> = None;
        let mut entries: Option<HashMap<Vec<u8>, Value, SeedableState<'static>>> = None;

        while let Some((key, value)) = map.next_entry::<String, FieldsEntries>()? {
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
            entries = Some(value.into_map());
        }

        let type_id = type_id.ok_or_else(|| de::Error::custom("Fields JSON missing type id"))?;
        let map = entries.unwrap_or_else(|| HashMap::with_hasher(SeedableState::default()));

        Ok(Fields { type_id, map })
    }
}

#[allow(exported_private_dependencies)]
impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(FieldsVisitor)
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
            Value::Text(string) | Value::String(string ) => serializer.serialize_str(string.as_str()),
            Value::Bytes(bytes) => serializer.serialize_bytes(bytes.as_slice()),
            Value::Bool(b) => serializer.serialize_bool(*b),
            Value::Fields(fields) => fields.serialize(serializer),
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
                Ok(Value::Text(value.to_owned()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Value::Text(value))
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
                let fields = Fields::deserialize(MapAccessDeserializer::new(map))?;
                Ok(Value::Fields(fields))
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
            Which::Fields(fields_reader) => {
                let fields_reader = fields_reader?;
                let type_id = fields_reader.get_type_id();
                let entries_reader = fields_reader.get_entries()?;
                let mut map = HashMap::with_capacity_and_hasher(
                    entries_reader.len() as usize,
                    SeedableState::default(),
                );
                for entry_reader in entries_reader.iter() {
                    let key = entry_reader.get_key()?.to_vec();
                    let value = Value::from_reader(entry_reader.get_value()?)?;
                    map.insert(key, value);
                }
                Ok(Value::Fields(Fields { type_id, map }))
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
            },
            Value::String(string) => {
                builder.set_string(string.as_str());
            }
            Value::Bytes(bytes) => {
                builder.set_bytes(bytes.as_slice());
            }
            Value::Bool(b) => {
                builder.set_bool(*b);
            }
            Value::Fields(fields) => {
                let mut fields_builder = builder.reborrow().init_fields();
                fields_builder.set_type_id(fields.type_id);
                let entry_len = fields.map.len();
                let mut entries_builder = fields_builder.reborrow().init_entries(entry_len as u32);
                for (index, (key, value)) in fields.map.iter().enumerate() {
                    let mut entry_builder = entries_builder.reborrow().get(index as u32);
                    entry_builder.set_key(key);
                    let value_builder = entry_builder.reborrow().init_value();
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
