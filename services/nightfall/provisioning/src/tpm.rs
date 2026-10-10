use std::time::Duration;

use dusk_capnp::capnp;
use rustls_pki_types::{CertificateDer, TrustAnchor, UnixTime};
use x509_parser::certificate::X509Certificate;
use x509_parser::prelude::FromDer;
use x509_parser::public_key::PublicKey;

use crate::credential::sha256;
use crate::provision_capnp::tpm_attestation;

pub const ENDORSEMENT_KEY_CERTIFICATE_USAGE: &[u8] = &[0x67, 0x81, 0x05, 0x08, 0x01];

const ENDORSEMENT_KEY_TEMPLATE: [u8; 58] = [
    0x00, 0x01, 0x00, 0x0b, 0x00, 0x03, 0x00, 0xb2, 0x00, 0x20, 0x83, 0x71, 0x97, 0x67, 0x44, 0x84,
    0xb3, 0xf8, 0x1a, 0x90, 0xcc, 0x8d, 0x46, 0xa5, 0xd7, 0x24, 0xfd, 0x52, 0xd7, 0x6e, 0x06, 0x52,
    0x0b, 0x64, 0xf2, 0xa1, 0xda, 0x1b, 0x33, 0x14, 0x69, 0xaa, 0x00, 0x06, 0x00, 0x80, 0x00, 0x43,
    0x00, 0x10, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
];
const ENDORSEMENT_KEY_MODULUS_BYTES: usize = 256;
const NODE_KEY_TEMPLATE: [u8; 20] = [
    0x00, 0x23, 0x00, 0x0b, 0x00, 0x04, 0x04, 0x72, 0x00, 0x00, 0x00, 0x10, 0x00, 0x18, 0x00, 0x0b,
    0x00, 0x03, 0x00, 0x10,
];
const P256_COORDINATE_BYTES: usize = 32;
const P256_PUBLIC_KEY_INFO: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];
const SHA256_ALGORITHM: [u8; 2] = [0x00, 0x0b];
const RSA_EXPONENT: [u8; 3] = [0x01, 0x00, 0x01];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub endorsement_key: Vec<u8>,
    pub endorsement_certificate: Vec<u8>,
    pub endorsement_certificate_chain: Vec<Vec<u8>>,
    pub node_key: Vec<u8>,
}

impl Evidence {
    pub fn read(reader: tpm_attestation::Reader<'_>) -> capnp::Result<Evidence> {
        Ok(Evidence {
            endorsement_key: reader.get_endorsement_key()?.to_vec(),
            endorsement_certificate: reader.get_endorsement_certificate()?.to_vec(),
            endorsement_certificate_chain: reader
                .get_endorsement_certificate_chain()?
                .iter()
                .map(|certificate| certificate.map(<[u8]>::to_vec))
                .collect::<capnp::Result<_>>()?,
            node_key: reader.get_node_key()?.to_vec(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attested {
    pub endorsement_modulus: Vec<u8>,
    pub node_key_name: Vec<u8>,
    pub node_key_public_key_info: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttestationError {
    #[error("this nightfall trusts no TPM endorsement key roots")]
    NoRoots,
    #[error("the TPM endorsement key has no certificate")]
    CertificateMissing,
    #[error("the TPM endorsement key does not follow TCG template L-1 (RSA 2048)")]
    EndorsementKeyTemplate,
    #[error("the TPM node key does not follow the node key template")]
    NodeKeyTemplate,
    #[error("the hardware fingerprint is not the SHA-256 of the TPM endorsement key")]
    Fingerprint,
    #[error("the TPM endorsement key certificate is not trusted: {0}")]
    Untrusted(String),
}

impl AttestationError {
    pub fn reason(&self) -> &'static str {
        match self {
            AttestationError::NoRoots => "tpm_roots_missing",
            AttestationError::CertificateMissing => "tpm_certificate_missing",
            AttestationError::EndorsementKeyTemplate | AttestationError::NodeKeyTemplate => {
                "tpm_key_template"
            }
            AttestationError::Fingerprint => "tpm_fingerprint_mismatch",
            AttestationError::Untrusted(_) => "tpm_certificate_untrusted",
        }
    }
}

fn endorsement_modulus(endorsement_key: &[u8]) -> Option<&[u8]> {
    endorsement_key
        .strip_prefix(&ENDORSEMENT_KEY_TEMPLATE)
        .filter(|modulus| modulus.len() == ENDORSEMENT_KEY_MODULUS_BYTES)
}

fn coordinate(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let (size, rest) = bytes.split_first_chunk::<2>()?;
    let size = usize::from(u16::from_be_bytes(*size));
    if !(1..=P256_COORDINATE_BYTES).contains(&size) {
        return None;
    }
    rest.split_at_checked(size)
}

fn node_key_point(node_key: &[u8]) -> Option<Vec<u8>> {
    let rest = node_key.strip_prefix(&NODE_KEY_TEMPLATE)?;
    let (x, rest) = coordinate(rest)?;
    let (y, rest) = coordinate(rest)?;
    if !rest.is_empty() {
        return None;
    }
    let mut point = [0u8; 1 + 2 * P256_COORDINATE_BYTES];
    point[0] = 0x04;
    point[1 + P256_COORDINATE_BYTES - x.len()..=P256_COORDINATE_BYTES].copy_from_slice(x);
    point[1 + 2 * P256_COORDINATE_BYTES - y.len()..].copy_from_slice(y);
    Some(point.to_vec())
}

fn without_leading_zeros(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len());
    &bytes[start..]
}

fn untrusted(reason: impl ToString) -> AttestationError {
    AttestationError::Untrusted(reason.to_string())
}

fn verify_certificate(
    evidence: &Evidence,
    modulus: &[u8],
    roots: &[TrustAnchor<'_>],
    now: UnixTime,
) -> Result<(), AttestationError> {
    let (_, parsed) =
        X509Certificate::from_der(&evidence.endorsement_certificate).map_err(untrusted)?;
    let leaf = CertificateDer::from(evidence.endorsement_certificate.as_slice());
    let end_entity = webpki::EndEntityCert::try_from(&leaf).map_err(untrusted)?;
    let intermediates: Vec<CertificateDer<'_>> = evidence
        .endorsement_certificate_chain
        .iter()
        .map(|certificate| CertificateDer::from(certificate.as_slice()))
        .collect();
    let verify_at = |time: UnixTime| {
        end_entity
            .verify_for_usage(
                webpki::ALL_VERIFICATION_ALGS,
                roots,
                &intermediates,
                time,
                webpki::KeyUsage::required_if_present(ENDORSEMENT_KEY_CERTIFICATE_USAGE),
                None,
                None,
            )
            .map(|_| ())
    };
    let verified = match verify_at(now) {
        Err(webpki::Error::CertExpired { .. }) => {
            let issued = u64::try_from(parsed.validity().not_before.timestamp()).unwrap_or(0);
            verify_at(UnixTime::since_unix_epoch(Duration::from_secs(issued)))
        }
        other => other,
    };
    verified.map_err(untrusted)?;
    let endorsement_usage = parsed
        .extended_key_usage()
        .map_err(untrusted)?
        .is_some_and(|usage| {
            usage
                .value
                .other
                .iter()
                .any(|oid| oid.as_bytes() == ENDORSEMENT_KEY_CERTIFICATE_USAGE)
        });
    if !endorsement_usage {
        return Err(untrusted(
            "it lacks the TCG endorsement key certificate usage 2.23.133.8.1",
        ));
    }
    match parsed.public_key().parsed() {
        Ok(PublicKey::RSA(key))
            if without_leading_zeros(key.modulus) == without_leading_zeros(modulus)
                && without_leading_zeros(key.exponent) == RSA_EXPONENT =>
        {
            Ok(())
        }
        _ => Err(untrusted("it certifies another key")),
    }
}

pub fn attest(
    evidence: &Evidence,
    hardware_fingerprint: &[u8],
    roots: &[TrustAnchor<'_>],
    now: UnixTime,
) -> Result<Attested, AttestationError> {
    if roots.is_empty() {
        return Err(AttestationError::NoRoots);
    }
    let modulus = endorsement_modulus(&evidence.endorsement_key)
        .ok_or(AttestationError::EndorsementKeyTemplate)?;
    let point = node_key_point(&evidence.node_key).ok_or(AttestationError::NodeKeyTemplate)?;
    if hardware_fingerprint != sha256(&evidence.endorsement_key) {
        return Err(AttestationError::Fingerprint);
    }
    if evidence.endorsement_certificate.is_empty() {
        return Err(AttestationError::CertificateMissing);
    }
    verify_certificate(evidence, modulus, roots, now)?;
    let mut node_key_name = SHA256_ALGORITHM.to_vec();
    node_key_name.extend_from_slice(&sha256(&evidence.node_key));
    let mut node_key_public_key_info = P256_PUBLIC_KEY_INFO.to_vec();
    node_key_public_key_info.extend_from_slice(&point);
    Ok(Attested {
        endorsement_modulus: modulus.to_vec(),
        node_key_name,
        node_key_public_key_info,
    })
}

#[cfg(test)]
pub(crate) mod fixtures {
    use rcgen::{
        CertificateParams, DistinguishedName, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
        KeyUsagePurpose, PublicKeyData, SignatureAlgorithm,
    };
    use time::OffsetDateTime;

    use super::*;

    pub(crate) const SWTPM_ENDORSEMENT_CERTIFICATE: &[u8] =
        include_bytes!("../testdata/swtpm-endorsement-certificate.der");
    pub(crate) const SWTPM_ISSUER: &[u8] = include_bytes!("../testdata/swtpm-issuer.der");
    pub(crate) const SWTPM_ROOT: &[u8] = include_bytes!("../testdata/swtpm-root.der");
    pub(crate) const GENERATOR_X: [u8; 32] = [
        0x6b, 0x17, 0xd1, 0xf2, 0xe1, 0x2c, 0x42, 0x47, 0xf8, 0xbc, 0xe6, 0xe5, 0x63, 0xa4, 0x40,
        0xf2, 0x77, 0x03, 0x7d, 0x81, 0x2d, 0xeb, 0x33, 0xa0, 0xf4, 0xa1, 0x39, 0x45, 0xd8, 0x98,
        0xc2, 0x96,
    ];
    pub(crate) const GENERATOR_Y: [u8; 32] = [
        0x4f, 0xe3, 0x42, 0xe2, 0xfe, 0x1a, 0x7f, 0x9b, 0x8e, 0xe7, 0xeb, 0x4a, 0x7c, 0x0f, 0x9e,
        0x16, 0x2b, 0xce, 0x33, 0x57, 0x6b, 0x31, 0x5e, 0xce, 0xcb, 0xb6, 0x40, 0x68, 0x37, 0xbf,
        0x51, 0xf5,
    ];

    pub(crate) fn endorsement_key_public(modulus: &[u8]) -> Vec<u8> {
        let mut public = ENDORSEMENT_KEY_TEMPLATE.to_vec();
        public.extend_from_slice(modulus);
        public
    }

    pub(crate) fn node_key_public(x: &[u8], y: &[u8]) -> Vec<u8> {
        let mut public = NODE_KEY_TEMPLATE.to_vec();
        for coordinate in [x, y] {
            public.extend_from_slice(&(coordinate.len() as u16).to_be_bytes());
            public.extend_from_slice(coordinate);
        }
        public
    }

    pub(crate) fn certificate_modulus(certificate: &[u8]) -> Vec<u8> {
        let (_, parsed) = X509Certificate::from_der(certificate).unwrap();
        let Ok(PublicKey::RSA(key)) = parsed.public_key().parsed() else {
            panic!("not an RSA certificate");
        };
        without_leading_zeros(key.modulus).to_vec()
    }

    struct RsaPublicKeyInfo(Vec<u8>);

    impl PublicKeyData for RsaPublicKeyInfo {
        fn der_bytes(&self) -> &[u8] {
            &self.0
        }

        fn algorithm(&self) -> &'static SignatureAlgorithm {
            &rcgen::PKCS_RSA_SHA256
        }
    }

    fn rsa_public_key_der(modulus: &[u8]) -> Vec<u8> {
        fn integer(bytes: &[u8]) -> Vec<u8> {
            let mut content = without_leading_zeros(bytes).to_vec();
            if content.first().is_some_and(|byte| *byte & 0x80 != 0) {
                content.insert(0, 0);
            }
            let mut encoded = vec![0x02];
            encoded.extend_from_slice(&length(content.len()));
            encoded.extend_from_slice(&content);
            encoded
        }
        fn length(length: usize) -> Vec<u8> {
            match length {
                0..=0x7f => vec![length as u8],
                0x80..=0xff => vec![0x81, length as u8],
                _ => vec![0x82, (length >> 8) as u8, length as u8],
            }
        }
        let mut body = integer(modulus);
        body.extend_from_slice(&integer(&RSA_EXPONENT));
        let mut sequence = vec![0x30];
        sequence.extend_from_slice(&length(body.len()));
        sequence.extend_from_slice(&body);
        sequence
    }

    pub(crate) struct Authority {
        pub(crate) certificate: Vec<u8>,
        issuer: Issuer<'static, KeyPair>,
    }

    impl Authority {
        pub(crate) fn root(name: &str) -> Authority {
            Authority::new(
                name,
                None,
                OffsetDateTime::now_utc() + time::Duration::days(3650),
            )
        }

        pub(crate) fn new(
            name: &str,
            parent: Option<&Authority>,
            not_after: OffsetDateTime,
        ) -> Authority {
            let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
            let mut distinguished_name = DistinguishedName::new();
            distinguished_name.push(rcgen::DnType::CommonName, name);
            params.distinguished_name = distinguished_name;
            params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
            params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
            params.not_before = OffsetDateTime::now_utc() - time::Duration::days(3650);
            params.not_after = not_after;
            let certificate = match parent {
                Some(parent) => params.signed_by(&key, &parent.issuer).unwrap(),
                None => params.self_signed(&key).unwrap(),
            };
            Authority {
                certificate: certificate.der().to_vec(),
                issuer: Issuer::new(params, key),
            }
        }

        pub(crate) fn endorsement_certificate(
            &self,
            modulus: &[u8],
            not_before: OffsetDateTime,
            not_after: OffsetDateTime,
            usage: bool,
        ) -> Vec<u8> {
            let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
            params.distinguished_name = DistinguishedName::new();
            params.key_usages = vec![KeyUsagePurpose::KeyEncipherment];
            if usage {
                params.extended_key_usages =
                    vec![ExtendedKeyUsagePurpose::Other(vec![2, 23, 133, 8, 1])];
            }
            params.not_before = not_before;
            params.not_after = not_after;
            params
                .signed_by(&RsaPublicKeyInfo(rsa_public_key_der(modulus)), &self.issuer)
                .unwrap()
                .der()
                .to_vec()
        }

        pub(crate) fn anchor(&self) -> TrustAnchor<'static> {
            webpki::anchor_from_trusted_cert(&CertificateDer::from(self.certificate.as_slice()))
                .unwrap()
                .to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;

    use super::fixtures::*;
    use super::*;

    fn swtpm_evidence() -> Evidence {
        Evidence {
            endorsement_key: endorsement_key_public(&certificate_modulus(
                SWTPM_ENDORSEMENT_CERTIFICATE,
            )),
            endorsement_certificate: SWTPM_ENDORSEMENT_CERTIFICATE.to_vec(),
            endorsement_certificate_chain: vec![SWTPM_ISSUER.to_vec()],
            node_key: node_key_public(&GENERATOR_X, &GENERATOR_Y),
        }
    }

    fn another_modulus() -> Vec<u8> {
        let mut modulus = certificate_modulus(SWTPM_ENDORSEMENT_CERTIFICATE);
        modulus[255] ^= 0x02;
        modulus
    }

    fn swtpm_root() -> TrustAnchor<'static> {
        webpki::anchor_from_trusted_cert(&CertificateDer::from(SWTPM_ROOT))
            .unwrap()
            .to_owned()
    }

    fn now() -> UnixTime {
        UnixTime::now()
    }

    #[test]
    fn accepts_the_endorsement_key_of_swtpm_and_names_its_node_key() {
        let evidence = swtpm_evidence();
        let fingerprint = sha256(&evidence.endorsement_key);
        assert_eq!(
            hex::encode(fingerprint),
            "02ce19865ee9eb948f563ab266f2a37f4f2eb12e3b15db272fcfe034166441b5"
        );
        let attested = attest(&evidence, &fingerprint, &[swtpm_root()], now()).unwrap();
        assert_eq!(
            attested.endorsement_modulus,
            certificate_modulus(SWTPM_ENDORSEMENT_CERTIFICATE)
        );
        assert_eq!(
            hex::encode(&attested.node_key_name),
            "000bb78abbc4c3a3d610134c32ee8cb3efb45bc08ca27acaa76bf06e531abf04d590"
        );
        let mut point = vec![0x04];
        point.extend_from_slice(&GENERATOR_X);
        point.extend_from_slice(&GENERATOR_Y);
        let (_, info) =
            x509_parser::x509::SubjectPublicKeyInfo::from_der(&attested.node_key_public_key_info)
                .unwrap();
        assert_eq!(info.subject_public_key.data.as_ref(), point.as_slice());
        assert_eq!(
            info.algorithm.parameters.unwrap().as_oid().unwrap(),
            x509_parser::oid_registry::OID_EC_P256
        );
    }

    #[test]
    fn refuses_evidence_while_no_root_is_configured() {
        let evidence = swtpm_evidence();
        let fingerprint = sha256(&evidence.endorsement_key);
        assert_eq!(
            attest(&evidence, &fingerprint, &[], now()),
            Err(AttestationError::NoRoots)
        );
    }

    #[test]
    fn refuses_an_endorsement_key_certificate_of_another_root() {
        let evidence = swtpm_evidence();
        let fingerprint = sha256(&evidence.endorsement_key);
        let stranger = Authority::root("another manufacturer");
        let refused = attest(&evidence, &fingerprint, &[stranger.anchor()], now()).unwrap_err();
        assert_eq!(refused.reason(), "tpm_certificate_untrusted");
        assert!(refused.to_string().contains("UnknownIssuer"), "{refused}");
        let without_chain = Evidence {
            endorsement_certificate_chain: Vec::new(),
            ..evidence
        };
        assert_eq!(
            attest(&without_chain, &fingerprint, &[swtpm_root()], now())
                .unwrap_err()
                .reason(),
            "tpm_certificate_untrusted"
        );
    }

    #[test]
    fn refuses_an_endorsement_key_without_a_certificate() {
        let evidence = Evidence {
            endorsement_certificate: Vec::new(),
            ..swtpm_evidence()
        };
        let fingerprint = sha256(&evidence.endorsement_key);
        assert_eq!(
            attest(&evidence, &fingerprint, &[swtpm_root()], now()),
            Err(AttestationError::CertificateMissing)
        );
    }

    #[test]
    fn refuses_a_certificate_of_another_key() {
        let evidence = Evidence {
            endorsement_key: endorsement_key_public(&another_modulus()),
            ..swtpm_evidence()
        };
        let fingerprint = sha256(&evidence.endorsement_key);
        let refused = attest(&evidence, &fingerprint, &[swtpm_root()], now()).unwrap_err();
        assert_eq!(
            refused,
            AttestationError::Untrusted(String::from("it certifies another key"))
        );
    }

    #[test]
    fn refuses_a_hardware_fingerprint_that_is_not_the_endorsement_key_digest() {
        let evidence = swtpm_evidence();
        assert_eq!(
            attest(&evidence, &[7u8; 32], &[swtpm_root()], now()),
            Err(AttestationError::Fingerprint)
        );
    }

    #[test]
    fn accepts_an_expired_but_otherwise_valid_chain() {
        let now_time = OffsetDateTime::now_utc();
        let root = Authority::root("manufacturer root");
        let intermediate = Authority::new(
            "manufacturing ca",
            Some(&root),
            now_time - time::Duration::days(30),
        );
        let modulus = another_modulus();
        let certificate = intermediate.endorsement_certificate(
            &modulus,
            now_time - time::Duration::days(800),
            now_time - time::Duration::days(400),
            true,
        );
        let evidence = Evidence {
            endorsement_key: endorsement_key_public(&modulus),
            endorsement_certificate: certificate,
            endorsement_certificate_chain: vec![intermediate.certificate.clone()],
            node_key: node_key_public(&GENERATOR_X, &GENERATOR_Y),
        };
        let fingerprint = sha256(&evidence.endorsement_key);
        let attested = attest(&evidence, &fingerprint, &[root.anchor()], now()).unwrap();
        assert_eq!(attested.endorsement_modulus, modulus);
    }

    #[test]
    fn refuses_a_certificate_that_is_not_valid_yet() {
        let now_time = OffsetDateTime::now_utc();
        let root = Authority::root("manufacturer root");
        let modulus = another_modulus();
        let evidence = Evidence {
            endorsement_key: endorsement_key_public(&modulus),
            endorsement_certificate: root.endorsement_certificate(
                &modulus,
                now_time + time::Duration::days(1),
                now_time + time::Duration::days(3650),
                true,
            ),
            endorsement_certificate_chain: Vec::new(),
            node_key: node_key_public(&GENERATOR_X, &GENERATOR_Y),
        };
        let fingerprint = sha256(&evidence.endorsement_key);
        let refused = attest(&evidence, &fingerprint, &[root.anchor()], now()).unwrap_err();
        assert!(refused.to_string().contains("CertNotValidYet"), "{refused}");
    }

    #[test]
    fn refuses_a_certificate_without_the_endorsement_key_usage() {
        let now_time = OffsetDateTime::now_utc();
        let root = Authority::root("manufacturer root");
        let modulus = another_modulus();
        let evidence = Evidence {
            endorsement_key: endorsement_key_public(&modulus),
            endorsement_certificate: root.endorsement_certificate(
                &modulus,
                now_time - time::Duration::days(1),
                now_time + time::Duration::days(3650),
                false,
            ),
            endorsement_certificate_chain: Vec::new(),
            node_key: node_key_public(&GENERATOR_X, &GENERATOR_Y),
        };
        let fingerprint = sha256(&evidence.endorsement_key);
        let refused = attest(&evidence, &fingerprint, &[root.anchor()], now()).unwrap_err();
        assert_eq!(
            refused,
            AttestationError::Untrusted(String::from(
                "it lacks the TCG endorsement key certificate usage 2.23.133.8.1"
            ))
        );
    }

    #[test]
    fn checks_the_endorsement_key_against_template_l1() {
        let evidence = swtpm_evidence();
        let mut altered = vec![evidence.endorsement_key.clone(); 4];
        altered[0][7] = 0x72;
        altered[1][43] ^= 0x01;
        altered[2].pop();
        altered[3].push(0);
        for endorsement_key in altered {
            let fingerprint = sha256(&endorsement_key);
            let evidence = Evidence {
                endorsement_key,
                ..swtpm_evidence()
            };
            assert_eq!(
                attest(&evidence, &fingerprint, &[swtpm_root()], now()),
                Err(AttestationError::EndorsementKeyTemplate)
            );
        }
    }

    #[test]
    fn checks_the_node_key_against_its_template() {
        let good = node_key_public(&GENERATOR_X, &GENERATOR_Y);
        let mut restricted = good.clone();
        restricted[5] = 0x05;
        let mut decrypt = good.clone();
        decrypt[5] = 0x06;
        let mut another_curve = good.clone();
        another_curve[17] = 0x04;
        let mut trailing = good.clone();
        trailing.push(0);
        let empty_coordinate = node_key_public(&[], &GENERATOR_Y);
        let long_coordinate = node_key_public(&[1u8; 33], &GENERATOR_Y);
        for node_key in [
            restricted,
            decrypt,
            another_curve,
            trailing,
            empty_coordinate,
            long_coordinate,
        ] {
            let evidence = Evidence {
                node_key,
                ..swtpm_evidence()
            };
            let fingerprint = sha256(&evidence.endorsement_key);
            assert_eq!(
                attest(&evidence, &fingerprint, &[swtpm_root()], now()),
                Err(AttestationError::NodeKeyTemplate)
            );
        }
        let point = node_key_point(&node_key_public(&GENERATOR_X[1..], &GENERATOR_Y)).unwrap();
        assert_eq!(point[..2], [0x04, 0x00]);
        assert_eq!(point[2..33], GENERATOR_X[1..]);
        assert_eq!(point[33..], GENERATOR_Y);
    }
}
