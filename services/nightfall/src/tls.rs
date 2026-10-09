use anyhow::{Context, bail};
use arc_swap::ArcSwap;
use rustls::client::danger::HandshakeSignatureValid;
use rustls::crypto::CryptoProvider;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::{ClientHello, ResolvesServerCert, WebPkiClientVerifier};
use rustls::sign::CertifiedKey;
use rustls::{
    DigitallySignedStruct, DistinguishedName, RootCertStore, ServerConfig, SignatureScheme,
};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::SystemTime;
use x509_parser::prelude::{FromDer, X509Certificate};

pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub fn load_certificates(path: &Path) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let certificates = CertificateDer::pem_file_iter(path)
        .with_context(|| format!("read the certificates in {}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("parse the certificates in {}", path.display()))?;
    if certificates.is_empty() {
        bail!("{} holds no certificate", path.display());
    }
    Ok(certificates)
}

pub fn load_key(path: &Path) -> anyhow::Result<PrivateKeyDer<'static>> {
    PrivateKeyDer::from_pem_file(path)
        .with_context(|| format!("read the private key in {}", path.display()))
}

fn modified(paths: &[PathBuf]) -> Vec<Option<SystemTime>> {
    paths
        .iter()
        .map(|path| {
            std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        })
        .collect()
}

pub trait Reloadable: Send + Sync {
    fn name(&self) -> &str;
    fn reload_if_changed(&self) -> anyhow::Result<bool>;
}

pub struct ReloadingCertificate {
    name: String,
    certificate: PathBuf,
    key: PathBuf,
    provider: Arc<CryptoProvider>,
    current: ArcSwap<CertifiedKey>,
    seen: Mutex<Vec<Option<SystemTime>>>,
}

impl std::fmt::Debug for ReloadingCertificate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReloadingCertificate")
            .field("name", &self.name)
            .field("certificate", &self.certificate)
            .finish()
    }
}

fn certified_key(
    certificate: &Path,
    key: &Path,
    provider: &CryptoProvider,
) -> anyhow::Result<CertifiedKey> {
    let chain = load_certificates(certificate)?;
    let key = load_key(key)?;
    let signing_key = provider
        .key_provider
        .load_private_key(key)
        .context("load the private key")?;
    let certified = CertifiedKey::new(chain, signing_key);
    certified
        .keys_match()
        .context("the private key does not belong to the certificate")?;
    Ok(certified)
}

impl ReloadingCertificate {
    pub fn load(
        name: &str,
        certificate: &Path,
        key: &Path,
    ) -> anyhow::Result<Arc<ReloadingCertificate>> {
        let provider = provider();
        let paths = [certificate.to_path_buf(), key.to_path_buf()];
        let seen = modified(&paths);
        let loaded = certified_key(certificate, key, &provider)
            .with_context(|| format!("load the {name} certificate"))?;
        Ok(Arc::new(ReloadingCertificate {
            name: name.to_string(),
            certificate: certificate.to_path_buf(),
            key: key.to_path_buf(),
            provider,
            current: ArcSwap::from_pointee(loaded),
            seen: Mutex::new(seen),
        }))
    }

    pub fn current(&self) -> Arc<CertifiedKey> {
        self.current.load_full()
    }
}

impl Reloadable for ReloadingCertificate {
    fn name(&self) -> &str {
        &self.name
    }

    fn reload_if_changed(&self) -> anyhow::Result<bool> {
        let paths = [self.certificate.clone(), self.key.clone()];
        let now = modified(&paths);
        let mut seen = self
            .seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *seen == now {
            return Ok(false);
        }
        let loaded = certified_key(&self.certificate, &self.key, &self.provider)?;
        self.current.store(Arc::new(loaded));
        *seen = now;
        Ok(true)
    }
}

impl ResolvesServerCert for ReloadingCertificate {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current.load_full())
    }
}

pub fn root_store(certificates: &[CertificateDer<'static>]) -> anyhow::Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    for certificate in certificates {
        roots
            .add(certificate.clone())
            .context("add a trust anchor")?;
    }
    Ok(roots)
}

pub struct ReloadingClientVerifier {
    name: String,
    path: PathBuf,
    mandatory: bool,
    provider: Arc<CryptoProvider>,
    current: ArcSwap<Arc<dyn ClientCertVerifier>>,
    certificates: ArcSwap<Vec<CertificateDer<'static>>>,
    seen: Mutex<Vec<Option<SystemTime>>>,
    apart_from: OnceLock<Weak<ReloadingClientVerifier>>,
}

impl std::fmt::Debug for ReloadingClientVerifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReloadingClientVerifier")
            .field("name", &self.name)
            .field("path", &self.path)
            .finish()
    }
}

fn build_verifier(
    certificates: &[CertificateDer<'static>],
    mandatory: bool,
    provider: &Arc<CryptoProvider>,
) -> anyhow::Result<Arc<dyn ClientCertVerifier>> {
    let roots = Arc::new(root_store(certificates)?);
    let builder = WebPkiClientVerifier::builder_with_provider(roots, provider.clone());
    let builder = if mandatory {
        builder
    } else {
        builder.allow_unauthenticated()
    };
    builder
        .build()
        .context("build the client certificate verifier")
}

impl ReloadingClientVerifier {
    pub fn load(
        name: &str,
        path: &Path,
        mandatory: bool,
    ) -> anyhow::Result<Arc<ReloadingClientVerifier>> {
        let provider = provider();
        let seen = modified(&[path.to_path_buf()]);
        let certificates =
            load_certificates(path).with_context(|| format!("load the {name} CA"))?;
        let verifier = build_verifier(&certificates, mandatory, &provider)?;
        Ok(Arc::new(ReloadingClientVerifier {
            name: name.to_string(),
            path: path.to_path_buf(),
            mandatory,
            provider,
            current: ArcSwap::from_pointee(verifier),
            certificates: ArcSwap::from_pointee(certificates),
            seen: Mutex::new(seen),
            apart_from: OnceLock::new(),
        }))
    }

    pub fn certificates(&self) -> Arc<Vec<CertificateDer<'static>>> {
        self.certificates.load_full()
    }

    pub fn keep_apart(
        first: &Arc<ReloadingClientVerifier>,
        second: &Arc<ReloadingClientVerifier>,
    ) -> anyhow::Result<()> {
        check_separate_trust(&first.certificates(), &second.certificates())?;
        if first.apart_from.set(Arc::downgrade(second)).is_err()
            || second.apart_from.set(Arc::downgrade(first)).is_err()
        {
            bail!("a client CA bundle is already kept apart from another");
        }
        Ok(())
    }

    fn inner(&self) -> Arc<Arc<dyn ClientCertVerifier>> {
        self.current.load_full()
    }
}

impl Reloadable for ReloadingClientVerifier {
    fn name(&self) -> &str {
        &self.name
    }

    fn reload_if_changed(&self) -> anyhow::Result<bool> {
        let now = modified(std::slice::from_ref(&self.path));
        let mut seen = self
            .seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *seen == now {
            return Ok(false);
        }
        let certificates = load_certificates(&self.path)?;
        if let Some(other) = self.apart_from.get().and_then(Weak::upgrade) {
            check_separate_trust(&certificates, &other.certificates()).with_context(|| {
                format!(
                    "{} now shares a certificate with the {} CA",
                    self.path.display(),
                    other.name
                )
            })?;
        }
        let verifier = build_verifier(&certificates, self.mandatory, &self.provider)?;
        self.current.store(Arc::new(verifier));
        self.certificates.store(Arc::new(certificates));
        *seen = now;
        Ok(true)
    }
}

impl ClientCertVerifier for ReloadingClientVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        self.mandatory
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        self.inner()
            .verify_client_cert(end_entity, intermediates, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner()
            .verify_tls12_signature(message, certificate, signature)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner()
            .verify_tls13_signature(message, certificate, signature)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner().supported_verify_schemes()
    }
}

pub fn server_config(
    certificate: Arc<dyn ResolvesServerCert>,
    verifier: Arc<dyn ClientCertVerifier>,
    tls12: bool,
) -> anyhow::Result<Arc<ServerConfig>> {
    let versions: &[&rustls::SupportedProtocolVersion] = if tls12 {
        &[&rustls::version::TLS13, &rustls::version::TLS12]
    } else {
        &[&rustls::version::TLS13]
    };
    let config = ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(versions)
        .context("choose the TLS versions")?
        .with_client_cert_verifier(verifier)
        .with_cert_resolver(certificate);
    Ok(Arc::new(config))
}

pub fn spki_set(certificates: &[CertificateDer<'_>]) -> anyhow::Result<HashSet<Vec<u8>>> {
    certificates
        .iter()
        .map(|certificate| {
            let (_, parsed) = X509Certificate::from_der(certificate.as_ref())
                .map_err(|error| anyhow::anyhow!("parse a CA certificate: {error}"))?;
            Ok(parsed.public_key().raw.to_vec())
        })
        .collect()
}

pub fn check_separate_trust(
    fleet: &[CertificateDer<'_>],
    inner: &[CertificateDer<'_>],
) -> anyhow::Result<()> {
    let fleet = spki_set(fleet)?;
    let inner = spki_set(inner)?;
    if !fleet.is_disjoint(&inner) {
        bail!(
            "fleet.client_ca and inner.client_ca share a certificate; nodes and principals must be signed by separate CAs"
        );
    }
    Ok(())
}

pub async fn reload_loop(
    reloadables: Vec<Arc<dyn Reloadable>>,
    period: std::time::Duration,
    stop: tokio_util::sync::CancellationToken,
) {
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            () = tokio::time::sleep(period) => {}
        }
        for reloadable in &reloadables {
            match reloadable.reload_if_changed() {
                Ok(true) => tracing::info!(name = reloadable.name(), "TLS material reloaded"),
                Ok(false) => {}
                Err(error) => {
                    metrics::counter!("nightfall_tls_reload_failures_total").increment(1);
                    tracing::warn!(
                        name = reloadable.name(),
                        error = format!("{error:#}"),
                        "TLS material changed but could not be loaded; the previous one stays in use"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rcgen::{
        BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
        KeyUsagePurpose, SanType,
    };

    pub(crate) struct Authority {
        pub certificate: rcgen::Certificate,
        pub issuer: Issuer<'static, KeyPair>,
    }

    impl Authority {
        pub(crate) fn new(name: &str) -> Authority {
            let mut parameters = CertificateParams::new(Vec::<String>::new()).unwrap();
            parameters
                .distinguished_name
                .push(rcgen::DnType::CommonName, name);
            parameters.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            parameters.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
            let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let certificate = parameters.self_signed(&key).unwrap();
            Authority {
                certificate,
                issuer: Issuer::new(parameters, key),
            }
        }

        pub(crate) fn pem(&self) -> String {
            self.certificate.pem()
        }

        pub(crate) fn issue(
            &self,
            dns: &[&str],
            uris: &[&str],
            client: bool,
        ) -> (
            CertificateDer<'static>,
            PrivateKeyDer<'static>,
            String,
            String,
        ) {
            let mut parameters =
                CertificateParams::new(dns.iter().map(|name| name.to_string()).collect::<Vec<_>>())
                    .unwrap();
            for uri in uris {
                parameters
                    .subject_alt_names
                    .push(SanType::URI(uri.to_string().try_into().unwrap()));
            }
            parameters.extended_key_usages = vec![if client {
                ExtendedKeyUsagePurpose::ClientAuth
            } else {
                ExtendedKeyUsagePurpose::ServerAuth
            }];
            let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let certificate = parameters.signed_by(&key, &self.issuer).unwrap();
            let der = certificate.der().clone();
            let key_der = PrivateKeyDer::Pkcs8(key.serialize_der().into());
            (der, key_der, certificate.pem(), key.serialize_pem())
        }
    }

    #[test]
    fn refuses_one_ca_trusted_on_both_sides() {
        let fleet = Authority::new("fleet-client");
        let internal = Authority::new("internal");
        let fleet_der = vec![fleet.certificate.der().clone()];
        let internal_der = vec![internal.certificate.der().clone()];
        assert!(check_separate_trust(&fleet_der, &internal_der).is_ok());
        let both = vec![
            internal.certificate.der().clone(),
            fleet.certificate.der().clone(),
        ];
        assert!(check_separate_trust(&fleet_der, &both).is_err());
    }

    #[test]
    fn refuses_a_reload_that_makes_the_two_client_trusts_share_a_ca() {
        let directory = tempfile::tempdir().unwrap();
        let fleet = Authority::new("fleet-client");
        let internal = Authority::new("internal");
        let fleet_path = directory.path().join("fleet-client-ca.crt");
        let internal_path = directory.path().join("internal-ca.crt");
        std::fs::write(&fleet_path, fleet.pem()).unwrap();
        std::fs::write(&internal_path, internal.pem()).unwrap();
        let fleet_clients =
            ReloadingClientVerifier::load("fleet client", &fleet_path, true).unwrap();
        let inner_clients =
            ReloadingClientVerifier::load("inner client", &internal_path, true).unwrap();
        ReloadingClientVerifier::keep_apart(&fleet_clients, &inner_clients).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&fleet_path, format!("{}{}", fleet.pem(), internal.pem())).unwrap();
        assert!(fleet_clients.reload_if_changed().is_err());
        assert_eq!(fleet_clients.certificates().len(), 1);
        let other_fleet = Authority::new("fleet-client-2");
        std::fs::write(&fleet_path, format!("{}{}", fleet.pem(), other_fleet.pem())).unwrap();
        assert!(fleet_clients.reload_if_changed().unwrap());
        assert_eq!(fleet_clients.certificates().len(), 2);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(
            &internal_path,
            format!("{}{}", internal.pem(), other_fleet.pem()),
        )
        .unwrap();
        assert!(inner_clients.reload_if_changed().is_err());
        assert_eq!(inner_clients.certificates().len(), 1);
    }

    #[test]
    fn reloads_a_changed_certificate_and_keeps_the_old_one_on_failure() {
        let directory = tempfile::tempdir().unwrap();
        let authority = Authority::new("fleet-server");
        let certificate_path = directory.path().join("fleet.crt");
        let key_path = directory.path().join("fleet.key");
        let (_, _, first_pem, first_key) = authority.issue(&["fleet.dusk.example"], &[], false);
        std::fs::write(&certificate_path, &first_pem).unwrap();
        std::fs::write(&key_path, &first_key).unwrap();
        let reloading = ReloadingCertificate::load("fleet", &certificate_path, &key_path).unwrap();
        let first = reloading.current().cert[0].clone();
        assert!(!reloading.reload_if_changed().unwrap());
        std::thread::sleep(std::time::Duration::from_millis(20));
        let (_, _, second_pem, second_key) = authority.issue(&["fleet.dusk.example"], &[], false);
        std::fs::write(&certificate_path, &second_pem).unwrap();
        std::fs::write(&key_path, &first_key).unwrap();
        assert!(reloading.reload_if_changed().is_err());
        assert_eq!(reloading.current().cert[0], first);
        std::fs::write(&key_path, &second_key).unwrap();
        assert!(reloading.reload_if_changed().unwrap());
        assert_ne!(reloading.current().cert[0], first);
        let ca_path = directory.path().join("ca.crt");
        std::fs::write(&ca_path, authority.pem()).unwrap();
        let verifier = ReloadingClientVerifier::load("fleet client", &ca_path, true).unwrap();
        assert!(verifier.client_auth_mandatory());
        assert_eq!(verifier.certificates().len(), 1);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&ca_path, "not a certificate").unwrap();
        assert!(verifier.reload_if_changed().is_err());
        assert_eq!(verifier.certificates().len(), 1);
        assert!(server_config(reloading, verifier, false).is_ok());
    }
}
