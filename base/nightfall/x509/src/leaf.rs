use crate::request::SUBJECT_ALTERNATIVE_NAME;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use der::asn1::{
    AnyRef, BitStringRef, GeneralizedTime, OctetStringRef, PrintableStringRef, UtcTime,
    Utf8StringRef,
};
use der::{Decode, Reader, SliceReader, Tag, TagNumber, Tagged};

const COMMON_NAME: &[u8] = &[0x55, 0x04, 0x03];

const VERSION: Tag = Tag::ContextSpecific {
    constructed: true,
    number: TagNumber::N0,
};
const EXTENSIONS: Tag = Tag::ContextSpecific {
    constructed: true,
    number: TagNumber::N3,
};
const OPTIONAL_FIELDS: [Tag; 3] = [
    Tag::ContextSpecific {
        constructed: false,
        number: TagNumber::N1,
    },
    Tag::ContextSpecific {
        constructed: false,
        number: TagNumber::N2,
    },
    EXTENSIONS,
];
const UNIFORM_RESOURCE_IDENTIFIER: Tag = Tag::ContextSpecific {
    constructed: false,
    number: TagNumber::N6,
};

pub struct Leaf {
    pub uris: Vec<String>,
    pub common_names: Vec<String>,
    pub not_before_unix_ms: u64,
    pub not_after_unix_ms: u64,
    pub public_key: Vec<u8>,
}

struct Fields<'a> {
    not_before_unix_ms: u64,
    not_after_unix_ms: u64,
    subject: AnyRef<'a>,
    public_key: &'a [u8],
    extensions: Vec<(&'a [u8], &'a [u8])>,
}

pub fn parse_leaf(der: &[u8]) -> Result<Leaf, String> {
    let fields = read_fields(der).map_err(|error| format!("not an X.509 certificate: {error}"))?;
    let mut names = fields
        .extensions
        .iter()
        .filter(|(identifier, _)| *identifier == SUBJECT_ALTERNATIVE_NAME)
        .map(|(_, value)| *value);
    let names = match (names.next(), names.next()) {
        (None, _) => return Err(String::from("no subject alternative names")),
        (Some(_), Some(_)) => {
            return Err(String::from(
                "more than one subject alternative name extension",
            ));
        }
        (Some(names), None) => AnyRef::from_der(names)
            .and_then(|names| names.sequence(elements))
            .map_err(|error| format!("unreadable subject alternative names: {error}"))?,
    };
    let uris = names
        .into_iter()
        .map(|name| {
            if name.tag() != UNIFORM_RESOURCE_IDENTIFIER {
                return Err(format!(
                    "a subject alternative name that is not a URI: {}",
                    name.tag()
                ));
            }
            core::str::from_utf8(name.value())
                .ok()
                .filter(|uri| uri.is_ascii())
                .map(ToString::to_string)
                .ok_or_else(|| String::from("a subject alternative name URI that is not ASCII"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let common_names = read_common_names(fields.subject)
        .map_err(|error| format!("unreadable common name: {error}"))?;
    Ok(Leaf {
        uris,
        common_names,
        not_before_unix_ms: fields.not_before_unix_ms,
        not_after_unix_ms: fields.not_after_unix_ms,
        public_key: fields.public_key.to_vec(),
    })
}

fn read_fields(der: &[u8]) -> der::Result<Fields<'_>> {
    let mut reader = SliceReader::new(der)?;
    let fields = reader.sequence(|certificate| {
        let fields = certificate.sequence(|fields| {
            if fields.peek_tag()? == VERSION && u8::from_der(AnyRef::decode(fields)?.value())? > 2 {
                return Err(Tag::Integer.value_error());
            }
            read_tagged(fields, Tag::Integer)?;
            read_tagged(fields, Tag::Sequence)?;
            read_tagged(fields, Tag::Sequence)?;
            let (not_before_unix_ms, not_after_unix_ms) =
                fields.sequence(|validity| Ok((time(validity)?, time(validity)?)))?;
            let subject = read_tagged(fields, Tag::Sequence)?;
            let public_key = fields.sequence(|information| {
                read_tagged(information, Tag::Sequence)?;
                Ok(BitStringRef::decode(information)?.raw_bytes())
            })?;
            let mut extensions = Vec::new();
            let mut next = 0;
            for field in elements(fields)? {
                next += 1 + OPTIONAL_FIELDS[next..]
                    .iter()
                    .position(|tag| *tag == field.tag())
                    .ok_or_else(|| field.tag().unexpected_error(None))?;
                if field.tag() == EXTENSIONS {
                    for extension in AnyRef::from_der(field.value())?.sequence(elements)? {
                        extensions.push(extension.sequence(|extension| {
                            let identifier = read_tagged(extension, Tag::ObjectIdentifier)?;
                            if extension.peek_tag()? == Tag::Boolean {
                                bool::decode(extension)?;
                            }
                            Ok((
                                identifier.value(),
                                OctetStringRef::decode(extension)?.as_bytes(),
                            ))
                        })?);
                    }
                }
            }
            Ok(Fields {
                not_before_unix_ms,
                not_after_unix_ms,
                subject,
                public_key,
                extensions,
            })
        })?;
        read_tagged(certificate, Tag::Sequence)?;
        BitStringRef::decode(certificate)?;
        Ok(fields)
    })?;
    reader.finish(fields)
}

fn elements<'a>(reader: &mut impl Reader<'a>) -> der::Result<Vec<AnyRef<'a>>> {
    let mut elements = Vec::new();
    while !reader.is_finished() {
        elements.push(AnyRef::decode(reader)?);
    }
    Ok(elements)
}

fn read_tagged<'a>(reader: &mut impl Reader<'a>, tag: Tag) -> der::Result<AnyRef<'a>> {
    let element = AnyRef::decode(reader)?;
    element.tag().assert_eq(tag)?;
    Ok(element)
}

fn time<'a>(reader: &mut impl Reader<'a>) -> der::Result<u64> {
    let since_1970 = match reader.peek_tag()? {
        Tag::UtcTime => UtcTime::decode(reader)?.to_unix_duration(),
        _ => GeneralizedTime::decode(reader)?.to_unix_duration(),
    };
    Ok(u64::try_from(since_1970.as_millis()).unwrap_or(u64::MAX))
}

fn read_common_names(subject: AnyRef<'_>) -> der::Result<Vec<String>> {
    let mut common_names = Vec::new();
    for names in subject.sequence(elements)? {
        names.tag().assert_eq(Tag::Set)?;
        let mut reader = SliceReader::new(names.value())?;
        for name in elements(&mut reader)? {
            let (identifier, value) = name.sequence(|name| {
                let identifier = read_tagged(name, Tag::ObjectIdentifier)?;
                Ok((identifier, AnyRef::decode(name)?))
            })?;
            if identifier.value() == COMMON_NAME {
                common_names.push(match value.tag() {
                    Tag::PrintableString => value
                        .decode_as::<PrintableStringRef<'_>>()?
                        .as_str()
                        .to_string(),
                    _ => value.decode_as::<Utf8StringRef<'_>>()?.as_str().to_string(),
                });
            }
        }
    }
    Ok(common_names)
}
