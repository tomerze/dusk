use std::prelude::rust_2024::*;

use std::sync::Arc;

use anyhow::Context as _;
use dusk_program::value::Value;
use dusk_program_kvs_internal::{FLAG_PERSISTENT, FLAG_SENSITIVE, FLAG_STICKY, Kvs, key_id};
use ring::rand::SecureRandom as _;
use rustls::pki_types::CertificateDer;
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::FromDer as _;

use crate::node_key::{CsrSigningKey, NodeKey, TlsSigningKey};

pub(crate) const DEVICE_URI_PREFIX: &str = "urn:dusk:device:";
pub(crate) const INSTALLATION_URI_PREFIX: &str = "urn:dusk:installation:";
const TENANT_URI_PREFIX: &str = "urn:dusk:tenant:";

const INSTALLATION_ID: u64 = key_id("nightfall.installation_id");
const PRIVATE_KEY: u64 = key_id("nightfall.private_key");
const STAGED_PRIVATE_KEY: u64 = key_id("nightfall.staged_private_key");
const CERTIFICATE_CHAIN: u64 = key_id("nightfall.certificate_chain");

const KEPT: u8 = FLAG_STICKY | FLAG_PERSISTENT;
const KEPT_SECRET: u8 = KEPT | FLAG_SENSITIVE;

const IDENTITY_KEY_NAMES: [&str; 4] = [
    "nightfall.installation_id",
    "nightfall.private_key",
    "nightfall.staged_private_key",
    "nightfall.certificate_chain",
];

pub(crate) static IDENTITY_KEYS: [u64; 4] = dusk_program_kvs_internal::key_ids(IDENTITY_KEY_NAMES);

#[cfg(feature = "client")]
dusk_program_kvs_internal::known_keys!(IDENTITY_KEY_LIST, &IDENTITY_KEY_NAMES);

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

pub(crate) fn certificate_request(
    device_id: &str,
    installation_id: &str,
    key: &NodeKey,
) -> anyhow::Result<Vec<u8>> {
    let mut parameters = rcgen::CertificateParams::default();
    parameters.distinguished_name = rcgen::DistinguishedName::new();
    parameters.subject_alt_names = vec![
        rcgen::SanType::URI(rcgen::string::Ia5String::try_from(format!(
            "{DEVICE_URI_PREFIX}{device_id}"
        ))?),
        rcgen::SanType::URI(rcgen::string::Ia5String::try_from(format!(
            "{INSTALLATION_URI_PREFIX}{installation_id}"
        ))?),
    ];
    let request = parameters.serialize_request(&CsrSigningKey(key))?;
    Ok(request.der().to_vec())
}

pub(crate) fn renew_point(not_before_unix_ms: u64, not_after_unix_ms: u64, random: u64) -> u64 {
    let lifetime = u128::from(not_after_unix_ms.saturating_sub(not_before_unix_ms));
    let earliest = lifetime * 55 / 100;
    let span = lifetime * 75 / 100 - earliest;
    let offset = earliest + u128::from(random) % (span + 1);
    not_before_unix_ms.saturating_add(u64::try_from(offset).unwrap_or(u64::MAX))
}

pub(crate) async fn installation_id(kvs: &Kvs) -> Option<String> {
    match kvs.get(INSTALLATION_ID).await? {
        Value::String(installation_id) if is_identifier(&installation_id) => Some(installation_id),
        _ => {
            tracing::error!(
                "nightfall.installation_id does not hold 32 lowercase hex digits; ignoring it"
            );
            None
        }
    }
}

async fn private_key(kvs: &Kvs, key: u64, name: &str) -> Option<(NodeKey, Vec<u8>)> {
    let Value::Bytes(pkcs8) = kvs.get(key).await? else {
        tracing::error!(name, "the key holds no bytes; ignoring it");
        return None;
    };
    match NodeKey::from_pkcs8(&pkcs8) {
        Ok(node_key) => Some((node_key, pkcs8)),
        Err(reason) => {
            tracing::error!(
                name,
                reason,
                "the stored private key is unusable; ignoring it"
            );
            None
        }
    }
}

fn certificate_chain(stored: Value) -> Option<Vec<CertificateDer<'static>>> {
    let chain = match stored {
        Value::List(certificates) => certificates
            .into_iter()
            .map(|certificate| match certificate {
                Value::Bytes(der) => Some(CertificateDer::from(der)),
                _ => None,
            })
            .collect::<Option<Vec<_>>>(),
        _ => None,
    };
    match chain {
        Some(chain) if !chain.is_empty() => Some(chain),
        _ => {
            tracing::error!(
                "nightfall.certificate_chain does not hold a list of certificates; ignoring it"
            );
            None
        }
    }
}

pub(crate) async fn load(kvs: &Kvs) -> anyhow::Result<Option<Identity>> {
    let Some(stored) = kvs.get(CERTIFICATE_CHAIN).await else {
        tracing::info!("the kvs holds no certificate");
        return Ok(None);
    };
    let Some(chain) = certificate_chain(stored) else {
        return Ok(None);
    };
    let leaf = match parse_leaf(&chain[0]) {
        Ok(leaf) => leaf,
        Err(reason) => {
            tracing::error!(reason, "the stored certificate is unusable");
            return Ok(None);
        }
    };
    let device_id = leaf.identity.device_id.clone();
    let installation_id = leaf.identity.installation_id.clone();
    let current = private_key(kvs, PRIVATE_KEY, "nightfall.private_key").await;
    let staged = private_key(kvs, STAGED_PRIVATE_KEY, "nightfall.staged_private_key").await;
    let key = match (current, staged) {
        (_, Some((staged, pkcs8))) if staged.public_key() == leaf.public_key.as_slice() => {
            kvs.set(PRIVATE_KEY, Value::Bytes(pkcs8), KEPT_SECRET)
                .await
                .context("couldn't store the staged private key as the current one")?;
            kvs.delete(STAGED_PRIVATE_KEY)
                .await
                .context("couldn't remove the staged private key")?;
            tracing::info!(
                device_id,
                installation_id,
                "finished a key replacement that was interrupted"
            );
            staged
        }
        (Some((current, _)), staged) if current.public_key() == leaf.public_key.as_slice() => {
            if staged.is_some() {
                kvs.delete(STAGED_PRIVATE_KEY)
                    .await
                    .context("couldn't remove the staged private key")?;
                tracing::info!(
                    device_id,
                    installation_id,
                    "discarded a private key a provisioning attempt left behind"
                );
            }
            current
        }
        _ => {
            tracing::error!(
                device_id,
                installation_id,
                "no stored private key matches the stored certificate"
            );
            return Ok(None);
        }
    };
    if self::installation_id(kvs).await.as_deref() != Some(installation_id.as_str()) {
        kvs.set(
            INSTALLATION_ID,
            Value::String(installation_id.clone()),
            KEPT,
        )
        .await
        .context("couldn't store the installation id")?;
        tracing::info!(
            device_id,
            installation_id,
            "stored the installation id of the stored certificate"
        );
    }
    Ok(Some(Identity {
        leaf,
        chain,
        key: Arc::new(key),
    }))
}

pub(crate) async fn stage(kvs: &Kvs, pkcs8: &[u8]) -> anyhow::Result<()> {
    kvs.set(
        STAGED_PRIVATE_KEY,
        Value::Bytes(pkcs8.to_vec()),
        KEPT_SECRET,
    )
    .await
    .context("couldn't stage the new private key in the kvs")
}

pub(crate) async fn store(kvs: &Kvs, identity: &Identity, pkcs8: &[u8]) -> anyhow::Result<()> {
    let chain = identity
        .chain
        .iter()
        .map(|certificate| Value::Bytes(certificate.to_vec()))
        .collect();
    kvs.set(CERTIFICATE_CHAIN, Value::List(chain), KEPT)
        .await
        .context("couldn't store the certificate chain")?;
    kvs.set(
        INSTALLATION_ID,
        Value::String(identity.installation_id().to_string()),
        KEPT,
    )
    .await
    .context("couldn't store the installation id")?;
    kvs.set(PRIVATE_KEY, Value::Bytes(pkcs8.to_vec()), KEPT_SECRET)
        .await
        .context("couldn't store the private key")?;
    kvs.delete(STAGED_PRIVATE_KEY)
        .await
        .context("couldn't remove the staged private key")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_nix as _;
    use futures::executor::block_on;
    use x509_parser::extensions::ParsedExtension;

    const DEVICE: &str = "00112233445566778899aabbccddeeff";
    const INSTALLATION: &str = "ffeeddccbbaa99887766554433221100";
    const ANOTHER_INSTALLATION: &str = "0123456789abcdef0123456789abcdef";

    struct PersistentKvs {
        kvs: Arc<Kvs>,
        tid: u64,
        path: std::path::PathBuf,
    }

    impl PersistentKvs {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "dusk-nightfall-identity-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let tid = dusk_core::driver::tid();
            dusk_program_kvs_internal::register_persistent(tid, path.to_str().unwrap()).unwrap();
            let mut namespace_id = [0u8; 8];
            ring::rand::SystemRandom::new()
                .fill(&mut namespace_id)
                .unwrap();
            let kvs = dusk_program_kvs_internal::get_kvs(u64::from_le_bytes(namespace_id));
            assert!(block_on(kvs.keeps_persistent_keys()));
            PersistentKvs { kvs, tid, path }
        }
    }

    impl Drop for PersistentKvs {
        fn drop(&mut self) {
            dusk_program_kvs_internal::release_persistent(self.tid);
            if let Err(error) = std::fs::remove_file(&self.path) {
                eprintln!("couldn't remove {}: {error}", self.path.display());
            }
        }
    }

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

        fn identity(&self, installation_id: &str) -> (Identity, Vec<u8>) {
            let (key, pkcs8) = NodeKey::generate().unwrap();
            let now = now_seconds();
            let der = self.issue(
                &[
                    &device_uri(),
                    &format!("{INSTALLATION_URI_PREFIX}{installation_id}"),
                ],
                None,
                &key,
                (now - 10, now + 3600),
            );
            let identity = Identity {
                leaf: parse_leaf(&der).unwrap(),
                chain: vec![der],
                key: Arc::new(key),
            };
            (identity, pkcs8)
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
    fn certificate_request_carries_exactly_the_two_uris_and_an_empty_subject() {
        let (key, _) = NodeKey::generate().unwrap();
        let der = certificate_request(DEVICE, INSTALLATION, &key).unwrap();
        let (remainder, request) =
            x509_parser::certification_request::X509CertificationRequest::from_der(&der).unwrap();
        assert!(remainder.is_empty());
        let information = &request.certification_request_info;
        assert_eq!(information.subject.iter().count(), 0);
        assert_eq!(
            information.subject_pki.subject_public_key.data.as_ref(),
            key.public_key()
        );
        let extensions: Vec<&ParsedExtension> = request.requested_extensions().unwrap().collect();
        assert_eq!(extensions.len(), 1);
        let ParsedExtension::SubjectAlternativeName(names) = extensions[0] else {
            std::panic!("expected only a subject alternative name extension, got {extensions:?}");
        };
        assert_eq!(
            names.general_names,
            [
                GeneralName::URI(&device_uri()),
                GeneralName::URI(&installation_uri())
            ]
        );
        ring::signature::UnparsedPublicKey::new(
            &ring::signature::ECDSA_P256_SHA256_ASN1,
            key.public_key(),
        )
        .verify(information.raw, request.signature_value.data.as_ref())
        .unwrap();
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

    #[test]
    fn a_stored_identity_loads_back_with_its_flags() {
        let store_under_test = PersistentKvs::new("load");
        let kvs = &store_under_test.kvs;
        assert!(block_on(load(kvs)).unwrap().is_none());
        let (identity, pkcs8) = Authority::new().identity(INSTALLATION);
        block_on(stage(kvs, &pkcs8)).unwrap();
        block_on(store(kvs, &identity, &pkcs8)).unwrap();
        let loaded = block_on(load(kvs)).unwrap().unwrap();
        assert_eq!(loaded.device_id(), DEVICE);
        assert_eq!(loaded.installation_id(), INSTALLATION);
        assert_eq!(loaded.key.public_key(), identity.key.public_key());
        assert_eq!(loaded.leaf.fingerprint, identity.leaf.fingerprint);
        loaded.certified_key().keys_match().unwrap();
        for (key, flags) in [
            (INSTALLATION_ID, KEPT),
            (CERTIFICATE_CHAIN, KEPT),
            (PRIVATE_KEY, KEPT_SECRET),
        ] {
            assert_eq!(block_on(kvs.get_with_flags(key)).unwrap().1, flags);
        }
        assert!(!block_on(kvs.exists(STAGED_PRIVATE_KEY)));
        assert_eq!(
            block_on(kvs.get(INSTALLATION_ID)),
            Some(Value::String(INSTALLATION.to_string()))
        );
    }

    #[test]
    fn a_key_replacement_cut_short_after_the_certificate_is_finished() {
        let store_under_test = PersistentKvs::new("interrupted");
        let kvs = &store_under_test.kvs;
        let authority = Authority::new();
        let (first, first_pkcs8) = authority.identity(INSTALLATION);
        block_on(store(kvs, &first, &first_pkcs8)).unwrap();
        let (second, second_pkcs8) = authority.identity(INSTALLATION);
        block_on(stage(kvs, &second_pkcs8)).unwrap();
        let chain = Value::List(vec![Value::Bytes(second.chain[0].to_vec())]);
        block_on(kvs.set(CERTIFICATE_CHAIN, chain, KEPT)).unwrap();
        let loaded = block_on(load(kvs)).unwrap().unwrap();
        assert_eq!(loaded.key.public_key(), second.key.public_key());
        assert!(!block_on(kvs.exists(STAGED_PRIVATE_KEY)));
        assert_eq!(
            block_on(kvs.get_with_flags(PRIVATE_KEY)),
            Some((Value::Bytes(second_pkcs8), KEPT_SECRET))
        );
    }

    #[test]
    fn a_key_left_by_a_failed_attempt_is_discarded() {
        let store_under_test = PersistentKvs::new("leftover");
        let kvs = &store_under_test.kvs;
        let (identity, pkcs8) = Authority::new().identity(INSTALLATION);
        block_on(store(kvs, &identity, &pkcs8)).unwrap();
        let (_, leftover) = NodeKey::generate().unwrap();
        block_on(stage(kvs, &leftover)).unwrap();
        let loaded = block_on(load(kvs)).unwrap().unwrap();
        assert_eq!(loaded.key.public_key(), identity.key.public_key());
        assert!(!block_on(kvs.exists(STAGED_PRIVATE_KEY)));
    }

    #[test]
    fn the_certificate_decides_the_installation_id() {
        let store_under_test = PersistentKvs::new("installation");
        let kvs = &store_under_test.kvs;
        let (identity, pkcs8) = Authority::new().identity(INSTALLATION);
        block_on(store(kvs, &identity, &pkcs8)).unwrap();
        block_on(kvs.set(
            INSTALLATION_ID,
            Value::String(ANOTHER_INSTALLATION.to_string()),
            KEPT,
        ))
        .unwrap();
        assert_eq!(
            block_on(installation_id(kvs)).as_deref(),
            Some(ANOTHER_INSTALLATION)
        );
        let loaded = block_on(load(kvs)).unwrap().unwrap();
        assert_eq!(loaded.installation_id(), INSTALLATION);
        assert_eq!(
            block_on(installation_id(kvs)).as_deref(),
            Some(INSTALLATION)
        );
    }

    #[test]
    fn an_unusable_identity_loads_as_none() {
        let store_under_test = PersistentKvs::new("unusable");
        let kvs = &store_under_test.kvs;
        let (identity, pkcs8) = Authority::new().identity(INSTALLATION);
        let (_, other_pkcs8) = NodeKey::generate().unwrap();
        for (key, value) in [
            (PRIVATE_KEY, Value::Bytes(other_pkcs8)),
            (PRIVATE_KEY, Value::Bytes(b"garbage".to_vec())),
            (PRIVATE_KEY, Value::String(String::from("not bytes"))),
            (CERTIFICATE_CHAIN, Value::List(Vec::new())),
            (
                CERTIFICATE_CHAIN,
                Value::List(vec![Value::Bytes(b"not a certificate".to_vec())]),
            ),
            (CERTIFICATE_CHAIN, Value::Bytes(identity.chain[0].to_vec())),
        ] {
            block_on(store(kvs, &identity, &pkcs8)).unwrap();
            assert!(block_on(load(kvs)).unwrap().is_some());
            block_on(kvs.set(key, value.clone(), KEPT)).unwrap();
            assert!(block_on(load(kvs)).unwrap().is_none(), "{value:?}");
        }
    }

    #[test]
    fn renew_points_fall_between_55_and_75_percent_of_the_lifetime() {
        let not_before = 1_700_000_000_000;
        let lifetime = 168 * 3600 * 1000;
        let not_after = not_before + lifetime;
        let earliest = not_before + lifetime * 55 / 100;
        let latest = not_before + lifetime * 75 / 100;
        let span = latest - earliest;
        assert_eq!(renew_point(not_before, not_after, 0), earliest);
        assert_eq!(renew_point(not_before, not_after, span), latest);
        assert_eq!(renew_point(not_before, not_after, span + 1), earliest);
        let mut buckets = [0u32; 20];
        for random in (0..100_000u64).map(|value| value.wrapping_mul(0x9e37_79b9_7f4a_7c15)) {
            let point = renew_point(not_before, not_after, random);
            assert!(point >= earliest && point <= latest, "{point}");
            let bucket = usize::try_from((point - earliest) * 20 / (span + 1)).unwrap();
            buckets[bucket] += 1;
        }
        for count in buckets {
            assert!((4_000..6_000).contains(&count), "{buckets:?}");
        }
    }

    #[test]
    fn renew_point_survives_degenerate_validity() {
        assert_eq!(renew_point(10, 5, 7), 10);
        assert_eq!(renew_point(u64::MAX - 1, u64::MAX, 200), u64::MAX - 1);
        assert_eq!(
            renew_point(0, u64::MAX, 200),
            u64::try_from(u128::from(u64::MAX) * 55 / 100).unwrap() + 200
        );
    }
}
