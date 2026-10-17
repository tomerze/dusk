use alloc::vec::Vec;
use der::Decode;
use der::asn1::{AnyRef, UintRef};

const SEQUENCE: u8 = 0x30;
const SET: u8 = 0x31;
const BOOLEAN: u8 = 0x01;
const INTEGER: u8 = 0x02;
const BIT_STRING: u8 = 0x03;
const OCTET_STRING: u8 = 0x04;
const OBJECT_IDENTIFIER: u8 = 0x06;
const ATTRIBUTES: u8 = 0xa0;
const UNIFORM_RESOURCE_IDENTIFIER: u8 = 0x86;

const EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const PRIME256V1: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const ECDSA_WITH_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const EXTENSION_REQUEST: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x0e];
pub(crate) const SUBJECT_ALTERNATIVE_NAME: &[u8] = &[0x55, 0x1d, 0x11];

fn element(tag: u8, parts: &[&[u8]]) -> Vec<u8> {
    let length: usize = parts.iter().map(|part| part.len()).sum();
    let mut encoded = Vec::with_capacity(length + 1 + 1 + size_of::<usize>());
    encoded.push(tag);
    if length < 0x80 {
        encoded.push(length as u8);
    } else {
        let bytes = length.to_be_bytes();
        let significant = &bytes[(length.leading_zeros() / 8) as usize..];
        encoded.push(0x80 | significant.len() as u8);
        encoded.extend_from_slice(significant);
    }
    for part in parts {
        encoded.extend_from_slice(part);
    }
    encoded
}

pub fn certificate_request(
    public_key: &[u8],
    uris: &[&str],
    sign: impl FnOnce(&[u8]) -> anyhow::Result<Vec<u8>>,
) -> anyhow::Result<Vec<u8>> {
    if uris.is_empty() {
        anyhow::bail!("a certificate request needs at least one URI");
    }
    if !matches!(
        (public_key.len(), public_key.first()),
        (65, Some(0x04)) | (33, Some(0x02 | 0x03))
    ) {
        anyhow::bail!("the public key is not a SEC1 P-256 point");
    }
    if let Some(uri) = uris.iter().find(|uri| !uri.is_ascii()) {
        anyhow::bail!("`{uri}` is not ASCII, which a URI in a certificate must be");
    }
    let public_key = element(
        SEQUENCE,
        &[
            &element(
                SEQUENCE,
                &[
                    &element(OBJECT_IDENTIFIER, &[EC_PUBLIC_KEY]),
                    &element(OBJECT_IDENTIFIER, &[PRIME256V1]),
                ],
            ),
            &element(BIT_STRING, &[&[0], public_key]),
        ],
    );
    let names: Vec<Vec<u8>> = uris
        .iter()
        .map(|uri| element(UNIFORM_RESOURCE_IDENTIFIER, &[uri.as_bytes()]))
        .collect();
    let names: Vec<&[u8]> = names.iter().map(Vec::as_slice).collect();
    let extension = element(
        SEQUENCE,
        &[
            &element(OBJECT_IDENTIFIER, &[SUBJECT_ALTERNATIVE_NAME]),
            &element(BOOLEAN, &[&[0xff]]),
            &element(OCTET_STRING, &[&element(SEQUENCE, &names)]),
        ],
    );
    let attribute = element(
        SEQUENCE,
        &[
            &element(OBJECT_IDENTIFIER, &[EXTENSION_REQUEST]),
            &element(SET, &[&element(SEQUENCE, &[&extension])]),
        ],
    );
    let information = element(
        SEQUENCE,
        &[
            &element(INTEGER, &[&[0]]),
            &element(SEQUENCE, &[]),
            &public_key,
            &element(ATTRIBUTES, &[&attribute]),
        ],
    );
    let signature = sign(&information)?;
    AnyRef::from_der(&signature)
        .and_then(|signature| {
            signature.sequence(|integers| {
                UintRef::decode(integers)?;
                UintRef::decode(integers)?;
                Ok(())
            })
        })
        .map_err(|error| {
            anyhow::anyhow!(
                "the signer returned something other than a DER ECDSA signature: {error}"
            )
        })?;
    Ok(element(
        SEQUENCE,
        &[
            &information,
            &element(
                SEQUENCE,
                &[&element(OBJECT_IDENTIFIER, &[ECDSA_WITH_SHA256])],
            ),
            &element(BIT_STRING, &[&[0], &signature]),
        ],
    ))
}
