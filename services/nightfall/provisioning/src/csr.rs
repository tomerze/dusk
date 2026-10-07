use std::collections::BTreeSet;

use x509_parser::certification_request::X509CertificationRequest;
use x509_parser::extensions::{GeneralName, ParsedExtension};
use x509_parser::oid_registry::{
    OID_EC_P256, OID_KEY_TYPE_EC_PUBLIC_KEY, OID_SIG_ECDSA_WITH_SHA256, OID_X509_COMMON_NAME,
};
use x509_parser::prelude::FromDer;

use crate::identity::{common_name, device_uri, installation_uri};

pub const MAXIMUM_CSR_BYTES: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CsrError {
    #[error("the CSR is larger than {MAXIMUM_CSR_BYTES} bytes")]
    TooLarge,
    #[error("the CSR is not a PKCS#10 DER certification request")]
    Malformed,
    #[error("the CSR key is not an ECDSA P-256 key")]
    KeyAlgorithm,
    #[error("the CSR is not signed with ecdsa-with-SHA256")]
    SignatureAlgorithm,
    #[error("the CSR self-signature does not verify")]
    Signature,
    #[error("the CSR subject must be empty or exactly CN={0}")]
    Subject(String),
    #[error("the CSR must name exactly the URIs {0} and {1} and nothing else")]
    SubjectAlternativeNames(String, String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCsr {
    pub common_name: Option<String>,
    pub public_key: Vec<u8>,
}

pub fn validate_csr(
    der: &[u8],
    device_id: &str,
    installation_id: &str,
) -> Result<ValidatedCsr, CsrError> {
    if der.len() > MAXIMUM_CSR_BYTES {
        return Err(CsrError::TooLarge);
    }
    let (rest, request) =
        X509CertificationRequest::from_der(der).map_err(|_| CsrError::Malformed)?;
    if !rest.is_empty() {
        return Err(CsrError::Malformed);
    }
    let request_info = &request.certification_request_info;
    let key_algorithm = &request_info.subject_pki.algorithm;
    let curve = key_algorithm
        .parameters
        .as_ref()
        .and_then(|parameters| parameters.as_oid().ok());
    if key_algorithm.algorithm != OID_KEY_TYPE_EC_PUBLIC_KEY || curve.as_ref() != Some(&OID_EC_P256)
    {
        return Err(CsrError::KeyAlgorithm);
    }
    if request.signature_algorithm.algorithm != OID_SIG_ECDSA_WITH_SHA256 {
        return Err(CsrError::SignatureAlgorithm);
    }
    request
        .verify_signature()
        .map_err(|_| CsrError::Signature)?;

    let expected_name = common_name(device_id, installation_id);
    let mut common_names = Vec::new();
    for attribute in request_info.subject.iter_attributes() {
        if attribute.attr_type() != &OID_X509_COMMON_NAME {
            return Err(CsrError::Subject(expected_name));
        }
        common_names.push(
            attribute
                .as_str()
                .map_err(|_| CsrError::Subject(expected_name.clone()))?,
        );
    }
    let common_name = match common_names.as_slice() {
        [] => None,
        [name] if *name == expected_name => Some(String::from(*name)),
        _ => return Err(CsrError::Subject(expected_name)),
    };

    let expected_device = device_uri(device_id);
    let expected_installation = installation_uri(installation_id);
    let names_error = || {
        CsrError::SubjectAlternativeNames(expected_device.clone(), expected_installation.clone())
    };
    let mut alternative_names = None;
    for extension in request.requested_extensions().into_iter().flatten() {
        if let ParsedExtension::SubjectAlternativeName(names) = extension {
            if alternative_names.is_some() {
                return Err(names_error());
            }
            alternative_names = Some(names);
        }
    }
    let names = alternative_names.ok_or_else(names_error)?;
    let mut uris = Vec::new();
    for name in &names.general_names {
        match name {
            GeneralName::URI(uri) => uris.push(*uri),
            _ => return Err(names_error()),
        }
    }
    let found: BTreeSet<&str> = uris.iter().copied().collect();
    let wanted: BTreeSet<&str> = [expected_device.as_str(), expected_installation.as_str()].into();
    if uris.len() != 2 || found != wanted {
        return Err(names_error());
    }
    Ok(ValidatedCsr {
        common_name,
        public_key: request_info.subject_pki.raw.to_vec(),
    })
}

#[cfg(test)]
pub(crate) mod fixtures {
    use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};

    pub(crate) fn csr(key: &KeyPair, common_name: Option<&str>, uris: &[String]) -> Vec<u8> {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        let mut name = DistinguishedName::new();
        if let Some(common_name) = common_name {
            name.push(DnType::CommonName, common_name);
        }
        params.distinguished_name = name;
        params.subject_alt_names = uris
            .iter()
            .map(|uri| SanType::URI(uri.as_str().try_into().unwrap()))
            .collect();
        params.serialize_request(key).unwrap().der().to_vec()
    }

    pub(crate) fn p256() -> KeyPair {
        KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use rcgen::{CertificateParams, DistinguishedName, DnType, SanType};

    use super::fixtures::{csr, p256};
    use super::*;

    const DEVICE: &str = "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13";
    const INSTALLATION: &str = "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70";

    fn uris() -> Vec<String> {
        vec![device_uri(DEVICE), installation_uri(INSTALLATION)]
    }

    #[test]
    fn accepts_a_csr_with_or_without_the_common_name() {
        let key = p256();
        let validated = validate_csr(&csr(&key, None, &uris()), DEVICE, INSTALLATION).unwrap();
        assert_eq!(validated.common_name, None);
        assert_eq!(
            validated.public_key,
            rcgen::PublicKeyData::subject_public_key_info(&key)
        );
        let name = common_name(DEVICE, INSTALLATION);
        let validated =
            validate_csr(&csr(&key, Some(&name), &uris()), DEVICE, INSTALLATION).unwrap();
        assert_eq!(validated.common_name.as_deref(), Some(name.as_str()));
        let reversed: Vec<String> = uris().into_iter().rev().collect();
        assert!(validate_csr(&csr(&key, None, &reversed), DEVICE, INSTALLATION).is_ok());
    }

    #[test]
    fn refuses_other_names_and_alternative_names() {
        let key = p256();
        let other = "00000000000000000000000000000000";
        let refused = [
            csr(&key, Some("someone-else"), &uris()),
            csr(&key, None, &[device_uri(DEVICE)]),
            csr(&key, None, &[device_uri(DEVICE), installation_uri(other)]),
            csr(
                &key,
                None,
                &[
                    device_uri(DEVICE),
                    installation_uri(INSTALLATION),
                    String::from("urn:dusk:tenant:retail"),
                ],
            ),
            csr(&key, None, &[device_uri(DEVICE), device_uri(DEVICE)]),
            csr(&key, None, &[]),
        ];
        for der in refused {
            assert!(validate_csr(&der, DEVICE, INSTALLATION).is_err());
        }
        let mut params = CertificateParams::new(vec![String::from("node.example")]).unwrap();
        params.distinguished_name = DistinguishedName::new();
        params.subject_alt_names.push(SanType::URI(
            device_uri(DEVICE).as_str().try_into().unwrap(),
        ));
        params.subject_alt_names.push(SanType::URI(
            installation_uri(INSTALLATION).as_str().try_into().unwrap(),
        ));
        let with_dns = params.serialize_request(&key).unwrap().der().to_vec();
        assert!(matches!(
            validate_csr(&with_dns, DEVICE, INSTALLATION),
            Err(CsrError::SubjectAlternativeNames(..))
        ));
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        let mut name = DistinguishedName::new();
        name.push(DnType::CommonName, common_name(DEVICE, INSTALLATION));
        name.push(DnType::OrganizationName, "dusk");
        params.distinguished_name = name;
        params.subject_alt_names = uris()
            .iter()
            .map(|uri| SanType::URI(uri.as_str().try_into().unwrap()))
            .collect();
        let with_organization = params.serialize_request(&key).unwrap().der().to_vec();
        assert!(matches!(
            validate_csr(&with_organization, DEVICE, INSTALLATION),
            Err(CsrError::Subject(_))
        ));
    }

    #[test]
    fn refuses_other_keys_forged_signatures_and_garbage() {
        let ed25519 = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
        assert_eq!(
            validate_csr(&csr(&ed25519, None, &uris()), DEVICE, INSTALLATION),
            Err(CsrError::KeyAlgorithm)
        );
        let p384 = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P384_SHA384).unwrap();
        assert_eq!(
            validate_csr(&csr(&p384, None, &uris()), DEVICE, INSTALLATION),
            Err(CsrError::KeyAlgorithm)
        );
        let mut forged = csr(&p256(), None, &uris());
        let last = forged.len() - 2;
        forged[last] ^= 0x01;
        assert_eq!(
            validate_csr(&forged, DEVICE, INSTALLATION),
            Err(CsrError::Signature)
        );
        assert_eq!(
            validate_csr(b"not a csr", DEVICE, INSTALLATION),
            Err(CsrError::Malformed)
        );
        let mut trailing = csr(&p256(), None, &uris());
        trailing.push(0);
        assert_eq!(
            validate_csr(&trailing, DEVICE, INSTALLATION),
            Err(CsrError::Malformed)
        );
        assert_eq!(
            validate_csr(&vec![0u8; MAXIMUM_CSR_BYTES + 1], DEVICE, INSTALLATION),
            Err(CsrError::TooLarge)
        );
    }
}
