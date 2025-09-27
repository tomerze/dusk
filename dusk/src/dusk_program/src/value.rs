#![allow(clippy::module_name_repetitions)]

extern crate alloc;

use alloc::{string::String, vec::Vec};
use core::{convert::TryFrom, fmt};

use capnp::Error;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use dusk_capnp::dusk_capnp::{field, value};
use nohash_hasher::BuildNoHashHasher;

pub const fn gen_id(data: &[u8]) -> u64 {
    const SEED: rapidhash::v3::RapidSecrets =
        rapidhash::v3::RapidSecrets::seed(dusk_capnp::dusk_capnp::ID_SEED);
    rapidhash::v3::rapidhash_v3_nano_inline::<true, true>(data, &SEED)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fields {
    // generated using `dusk_program::value::gen_id`
    pub type_id: u64,
    // keys are generated using `dusk_program::value::gen_id`
    pub map: hashbrown::HashMap<u64, Value, BuildNoHashHasher<u64>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Uint(u64),
    Text(String),
    Bytes(Vec<u8>),
    Bool(bool),
    Fields(Fields),
    List(Vec<Value>),
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
            Value::Text(text) => serializer.serialize_str(text.as_str()),
            Value::Bytes(bytes) => serializer.serialize_bytes(bytes.as_slice()),
            Value::Bool(b) => serializer.serialize_bool(*b),
            Value::Fields(fields) => {
                let mut map = serializer.serialize_map(Some(fields.len()))?;
                for field in fields {
                    map.serialize_entry(&field.key, &field.value)?;
                }
                map.end()
            }
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

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut fields = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Value>()? {
                    fields.push(Field { key, value });
                }
                Ok(Value::Fields(fields))
            }
        }

        deserializer.deserialize_any(ValueVisitor)
    }
}

impl Field {
    /// Decode a `Field` from its Cap'n Proto representation.
    pub fn from_reader(reader: field::Reader<'_>) -> Result<Self, Error> {
        let key_reader = reader.get_key()?;
        let key = key_reader
            .to_str()
            .map_err(|err| Error::failed(err.to_string()))?
            .to_owned();
        let value_reader = reader.get_value()?;
        let value = Value::from_reader(value_reader)?;
        Ok(Field { key, value })
    }

    /// Encode this `Field` into a Cap'n Proto builder.
    pub fn write_to_builder<'a>(&self, mut builder: field::Builder<'a>) -> Result<(), Error> {
        builder.set_key(&self.key);
        let value_builder = builder.reborrow().init_value();
        self.value.write_to_builder(value_builder)?;
        Ok(())
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
            Which::Bytes(data_reader) => Ok(Value::Bytes(data_reader?.to_vec())),
            Which::Bool(b) => Ok(Value::Bool(b)),
            Which::Fields(list_reader) => {
                let list_reader = list_reader?;
                let mut fields = Vec::with_capacity(list_reader.len() as usize);
                for field_reader in list_reader.iter() {
                    fields.push(Field::from_reader(field_reader)?);
                }
                Ok(Value::Fields(fields))
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
            Value::Bytes(bytes) => {
                builder.set_bytes(bytes.as_slice());
            }
            Value::Bool(b) => {
                builder.set_bool(*b);
            }
            Value::Fields(fields) => {
                // let mut fields_builder = builder.reborrow().init_fields(fields.len() as u32);
                // for (index, field) in fields.iter().enumerate() {
                //     let field_builder = fields_builder.reborrow().get(index as u32);
                //     field.write_to_builder(field_builder)?;
                // }
                let fields_builder = builder.reborrow().init_fields();
                fields_builder.set_type_id(value);
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

impl<'a> TryFrom<field::Reader<'a>> for Field {
    type Error = Error;

    fn try_from(reader: field::Reader<'a>) -> Result<Self, Self::Error> {
        Field::from_reader(reader)
    }
}

impl<'a> TryFrom<value::Reader<'a>> for Value {
    type Error = Error;

    fn try_from(reader: value::Reader<'a>) -> Result<Self, Self::Error> {
        Value::from_reader(reader)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_field(value: Value, key: &str) -> Value {
        match value {
            Value::Fields(fields) => fields
                .into_iter()
                .find(|field| field.key == key)
                .map(|field| field.value)
                .expect("missing field"),
            _ => panic!("expected object value"),
        }
    }

    #[test]
    fn json_object_positive_int_becomes_uint() {
        let value = Value::from_json_str(r#"{"pid": 42}"#).expect("json parse");
        let pid = extract_field(value, "pid");
        assert!(matches!(pid, Value::Uint(42)));
    }

    #[test]
    fn json_nested_roundtrip_preserves_shapes() {
        let json = r#"{"outer": {"inner": [1, {"flag": true}]}}"#;
        let value = Value::from_json_str(json).expect("json parse");
        let roundtrip = value.to_json_string().expect("json serialize");
        let original = serde_json::from_str::<serde_json::Value>(json).unwrap();
        let reparsed = serde_json::from_str::<serde_json::Value>(&roundtrip).unwrap();
        assert_eq!(original, reparsed);

        let inner_list = match extract_field(value, "outer") {
            Value::Fields(fields) => extract_field(Value::Fields(fields), "inner"),
            _ => panic!("expected object"),
        };
        match inner_list {
            Value::List(items) => {
                assert!(matches!(items[0], Value::Uint(1)));
                let nested = match &items[1] {
                    Value::Fields(fields) => fields,
                    _ => panic!("expected object"),
                };
                assert!(matches!(nested[0].value, Value::Bool(true)));
            }
            _ => panic!("expected list"),
        }
    }

    #[test]
    fn serialize_positive_int_value_is_allowed() {
        let json = Value::Uint(7).to_json_string().expect("json serialize");
        assert_eq!(json, "7");
    }
}
