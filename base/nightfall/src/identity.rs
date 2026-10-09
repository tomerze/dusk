use std::prelude::rust_2024::*;

use std::sync::Arc;

use rustls::pki_types::CertificateDer;
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::FromDer as _;

use crate::node_key::{CsrSigningKey, NodeKey, TlsSigningKey};

pub(crate) const DEVICE_URI_PREFIX: &str = "urn:dusk:device:";
pub(crate) const INSTALLATION_URI_PREFIX: &str = "urn:dusk:installation:";
const TENANT_URI_PREFIX: &str = "urn:dusk:tenant:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CertificateIdentity {
    pub(crate) device_id: String,
    pub(crate) installation_id: String,
    pub(crate) tenant: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct LeafCertificate {
    pub(crate) identity: CertificateIdentity,
    pub(crate) public_key: Vec<u8>,
    pub(crate) not_before_unix_ms: u64,
    pub(crate) not_after_unix_ms: u64,
    pub(crate) fingerprint: String,
}

#[derive(Clone)]
pub(crate) struct Identity {
    pub(crate) leaf: LeafCertificate,
    pub(crate) chain: Vec<CertificateDer<'static>>,
    pub(crate) key: Arc<NodeKey>,
}

impl core::fmt::Debug for Identity {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Identity")
            .field("leaf", &self.leaf)
            .field("chain_length", &self.chain.len())
            .finish()
    }
}

impl Identity {
    pub(crate) fn certified_key(&self) -> Arc<rustls::sign::CertifiedKey> {
        Arc::new(rustls::sign::CertifiedKey::new(
            self.chain.clone(),
            Arc::new(TlsSigningKey::new(self.key.clone())),
        ))
    }

    pub(crate) fn device_id(&self) -> &str {
        &self.leaf.identity.device_id
    }

    pub(crate) fn installation_id(&self) -> &str {
        &self.leaf.identity.installation_id
    }

    pub(crate) fn is_expired(&self, now_unix_ms: u64) -> bool {
        now_unix_ms >= self.leaf.not_after_unix_ms
    }
}

pub(crate) fn is_identifier(text: &str) -> bool {
    text.len() == 32
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

pub(crate) fn identity_from_names<'name>(
    names: impl IntoIterator<Item = &'name GeneralName<'name>>,
) -> Result<CertificateIdentity, String> {
    let mut device_id = None;
    let mut installation_id = None;
    let mut tenant = None;
    for name in names {
        let GeneralName::URI(uri) = name else {
            return Err(format!("unexpected subject alternative name {name}"));
        };
        if let Some(value) = uri.strip_prefix(DEVICE_URI_PREFIX) {
            if !is_identifier(value) {
                return Err(format!(
                    "`{uri}` does not name a 32 lowercase hex device id"
                ));
            }
            if device_id.replace(value.to_string()).is_some() {
                return Err(String::from("more than one device id"));
            }
        } else if let Some(value) = uri.strip_prefix(INSTALLATION_URI_PREFIX) {
            if !is_identifier(value) {
                return Err(format!(
                    "`{uri}` does not name a 32 lowercase hex installation id"
                ));
            }
            if installation_id.replace(value.to_string()).is_some() {
                return Err(String::from("more than one installation id"));
            }
        } else if let Some(value) = uri.strip_prefix(TENANT_URI_PREFIX) {
            if !is_tenant(value) {
                return Err(format!("`{uri}` does not name a tenant"));
            }
            if tenant.replace(value.to_string()).is_some() {
                return Err(String::from("more than one tenant"));
            }
        } else {
            return Err(format!("unexpected subject alternative name URI `{uri}`"));
        }
    }
    Ok(CertificateIdentity {
        device_id: device_id.ok_or_else(|| String::from("no device id"))?,
        installation_id: installation_id.ok_or_else(|| String::from("no installation id"))?,
        tenant,
    })
}

fn is_tenant(text: &str) -> bool {
    (1..=63).contains(&text.len())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(crate) fn parse_leaf(der: &[u8]) -> Result<LeafCertificate, String> {
    let (remainder, certificate) = x509_parser::certificate::X509Certificate::from_der(der)
        .map_err(|error| format!("not an X.509 certificate: {error}"))?;
    if !remainder.is_empty() {
        return Err(String::from("trailing bytes after the certificate"));
    }
    let names = certificate
        .subject_alternative_name()
        .map_err(|error| format!("unreadable subject alternative names: {error}"))?
        .ok_or_else(|| String::from("no subject alternative names"))?;
    let identity = identity_from_names(&names.value.general_names)?;
    let expected_common_name = format!("{}.{}", identity.device_id, identity.installation_id);
    for common_name in certificate.subject().iter_common_name() {
        let common_name = common_name
            .as_str()
            .map_err(|error| format!("unreadable common name: {error}"))?;
        if !common_name.is_empty() && common_name != expected_common_name {
            return Err(format!(
                "common name `{common_name}` is neither empty nor `{expected_common_name}`"
            ));
        }
    }
    let validity = certificate.validity();
    let not_before_unix_ms = unix_milliseconds(validity.not_before.timestamp())?;
    let not_after_unix_ms = unix_milliseconds(validity.not_after.timestamp())?;
    if not_after_unix_ms <= not_before_unix_ms {
        return Err(String::from("it expires before it becomes valid"));
    }
    Ok(LeafCertificate {
        identity,
        public_key: certificate
            .public_key()
            .subject_public_key
            .data
            .as_ref()
            .to_vec(),
        not_before_unix_ms,
        not_after_unix_ms,
        fingerprint: encode_hex(ring::digest::digest(&ring::digest::SHA256, der).as_ref()),
    })
}

fn unix_milliseconds(seconds: i64) -> Result<u64, String> {
    u64::try_from(seconds)
        .ok()
        .and_then(|seconds| seconds.checked_mul(1000))
        .ok_or_else(|| format!("validity time {seconds} is before 1970"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE: &str = "00112233445566778899aabbccddeeff";
    const INSTALLATION: &str = "ffeeddccbbaa99887766554433221100";

    struct Authority {
        issuer: rcgen::Issuer<'static, rcgen::KeyPair>,
    }

    impl Authority {
        fn new() -> Self {
            let mut parameters = rcgen::CertificateParams::default();
            parameters.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
            let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            Authority {
                issuer: rcgen::Issuer::new(parameters, key),
            }
        }

        fn issue(
            &self,
            names: &[&str],
            common_name: Option<&str>,
            key: &NodeKey,
            validity_seconds: (i64, i64),
        ) -> CertificateDer<'static> {
            let mut parameters = rcgen::CertificateParams::default();
            parameters.distinguished_name = rcgen::DistinguishedName::new();
            if let Some(common_name) = common_name {
                parameters
                    .distinguished_name
                    .push(rcgen::DnType::CommonName, common_name);
            }
            parameters.subject_alt_names = names
                .iter()
                .map(|name| {
                    if let Some(dns) = name.strip_prefix("dns:") {
                        rcgen::SanType::DnsName(rcgen::string::Ia5String::try_from(dns).unwrap())
                    } else {
                        rcgen::SanType::URI(rcgen::string::Ia5String::try_from(*name).unwrap())
                    }
                })
                .collect();
            parameters.not_before = time_from_unix(validity_seconds.0);
            parameters.not_after = time_from_unix(validity_seconds.1);
            parameters
                .signed_by(&CsrSigningKey(key), &self.issuer)
                .unwrap()
                .der()
                .clone()
        }
    }

    fn time_from_unix(seconds: i64) -> time::OffsetDateTime {
        time::OffsetDateTime::from_unix_timestamp(seconds).unwrap()
    }

    fn now_seconds() -> i64 {
        i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
        .unwrap()
    }

    fn device_uri() -> String {
        format!("{DEVICE_URI_PREFIX}{DEVICE}")
    }

    fn installation_uri() -> String {
        format!("{INSTALLATION_URI_PREFIX}{INSTALLATION}")
    }

    fn uris(names: &[&str]) -> Vec<GeneralName<'static>> {
        names
            .iter()
            .map(|name| GeneralName::URI(Box::leak(name.to_string().into_boxed_str())))
            .collect()
    }

    #[test]
    fn subject_alternative_names_accept_device_installation_and_an_optional_tenant() {
        let identity = identity_from_names(&uris(&[&device_uri(), &installation_uri()])).unwrap();
        assert_eq!(
            identity,
            CertificateIdentity {
                device_id: DEVICE.to_string(),
                installation_id: INSTALLATION.to_string(),
                tenant: None
            }
        );
        let identity = identity_from_names(&uris(&[
            &installation_uri(),
            "urn:dusk:tenant:acme-1",
            &device_uri(),
        ]))
        .unwrap();
        assert_eq!(identity.tenant.as_deref(), Some("acme-1"));
    }

    #[test]
    fn subject_alternative_names_reject_anything_else() {
        let device = device_uri();
        let installation = installation_uri();
        let cases: Vec<Vec<String>> = vec![
            vec![device.clone()],
            vec![installation.clone()],
            vec![device.clone(), device.clone(), installation.clone()],
            vec![device.clone(), installation.clone(), installation.clone()],
            vec![
                format!("{DEVICE_URI_PREFIX}{}", DEVICE.to_uppercase()),
                installation.clone(),
            ],
            vec![format!("{DEVICE_URI_PREFIX}abc"), installation.clone()],
            vec![
                device.clone(),
                format!("{INSTALLATION_URI_PREFIX}{INSTALLATION}0"),
            ],
            vec![
                device.clone(),
                installation.clone(),
                "urn:dusk:tenant:".into(),
            ],
            vec![
                device.clone(),
                installation.clone(),
                "urn:dusk:tenant:Acme".into(),
            ],
            vec![
                device.clone(),
                installation.clone(),
                format!("urn:dusk:tenant:{}", "a".repeat(64)),
            ],
            vec![
                device.clone(),
                installation.clone(),
                "urn:dusk:tenant:a".into(),
                "urn:dusk:tenant:b".into(),
            ],
            vec![
                device.clone(),
                installation.clone(),
                "urn:dusk:principal:dawn-0".into(),
            ],
            vec![
                device.clone(),
                installation.clone(),
                "https://example.com".into(),
            ],
        ];
        for case in cases {
            let names: Vec<&str> = case.iter().map(String::as_str).collect();
            assert!(identity_from_names(&uris(&names)).is_err(), "{case:?}");
        }
        let mut names = uris(&[&device, &installation]);
        names.push(GeneralName::DNSName("node.example"));
        assert!(identity_from_names(&names).is_err());
        let mut names = uris(&[&device, &installation]);
        names.push(GeneralName::IPAddress(&[127, 0, 0, 1]));
        assert!(identity_from_names(&names).is_err());
    }

    #[test]
    fn parse_leaf_reads_identity_validity_and_key() {
        let (key, _) = NodeKey::generate().unwrap();
        let authority = Authority::new();
        let now = now_seconds();
        let der = authority.issue(
            &[&device_uri(), &installation_uri()],
            Some(&format!("{DEVICE}.{INSTALLATION}")),
            &key,
            (now - 10, now + 3600),
        );
        let leaf = parse_leaf(&der).unwrap();
        assert_eq!(leaf.identity.device_id, DEVICE);
        assert_eq!(leaf.identity.installation_id, INSTALLATION);
        assert_eq!(leaf.public_key, key.public_key());
        assert_eq!(
            leaf.not_before_unix_ms,
            u64::try_from(now - 10).unwrap() * 1000
        );
        assert_eq!(
            leaf.not_after_unix_ms,
            u64::try_from(now + 3600).unwrap() * 1000
        );
        assert_eq!(leaf.fingerprint.len(), 64);
        let without_common_name = authority.issue(
            &[&device_uri(), &installation_uri()],
            None,
            &key,
            (now, now + 60),
        );
        assert!(parse_leaf(&without_common_name).is_ok());
    }

    #[test]
    fn parse_leaf_rejects_a_wrong_common_name_or_extra_names() {
        let (key, _) = NodeKey::generate().unwrap();
        let authority = Authority::new();
        let now = now_seconds();
        let wrong_common_name = authority.issue(
            &[&device_uri(), &installation_uri()],
            Some("node.example"),
            &key,
            (now, now + 60),
        );
        assert!(parse_leaf(&wrong_common_name).is_err());
        let extra_name = authority.issue(
            &[&device_uri(), &installation_uri(), "dns:node.example"],
            None,
            &key,
            (now, now + 60),
        );
        assert!(parse_leaf(&extra_name).is_err());
        let backwards = authority.issue(
            &[&device_uri(), &installation_uri()],
            None,
            &key,
            (now + 60, now),
        );
        assert!(parse_leaf(&backwards).is_err());
        assert!(parse_leaf(b"not a certificate").is_err());
    }

}
