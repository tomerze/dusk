use anyhow::Context;
use rcgen::{
    CertificateParams, DistinguishedName, ExtendedKeyUsagePurpose, KeyPair, KeyUsagePurpose,
    SanType,
};
use rustls::client::{Resumption, WebPkiServerVerifier};
use rustls::crypto::CryptoProvider;
use rustls::{ClientConfig, RootCertStore};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

pub const BACKDATE: Duration = Duration::from_secs(60);
pub const RENEW_BEFORE: Duration = Duration::from_secs(3600);

pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub fn device_id(simulator: u64, link: u64) -> String {
    format!("{simulator:016x}{link:016x}")
}

pub fn installation_id(device_id: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, device_id.as_bytes()).as_ref()[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn load_certificates(path: &Path) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let certificates = CertificateDer::pem_file_iter(path)
        .with_context(|| format!("read the certificates in {}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("parse the certificates in {}", path.display()))?;
    anyhow::ensure!(
        !certificates.is_empty(),
        "{} holds no certificate",
        path.display()
    );
    Ok(certificates)
}

pub struct Minted {
    pub chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
    pub not_after: SystemTime,
}

pub struct Identity {
    pub config: Arc<ClientConfig>,
    pub not_after: SystemTime,
}

impl Identity {
    pub fn renewal_due(&self) -> bool {
        SystemTime::now() + RENEW_BEFORE >= self.not_after
    }
}

pub struct Issuer {
    certificate: CertificateDer<'static>,
    issuer: rcgen::Issuer<'static, KeyPair>,
    verifier: Arc<WebPkiServerVerifier>,
    lifetime: Duration,
    tenant: Option<String>,
}

impl Issuer {
    pub fn load(
        certificate: &Path,
        key: &Path,
        fleet_server_ca: &Path,
        lifetime: Duration,
        tenant: Option<String>,
    ) -> anyhow::Result<Issuer> {
        let certificate_pem = std::fs::read_to_string(certificate)
            .with_context(|| format!("read {}", certificate.display()))?;
        let key_pem =
            std::fs::read_to_string(key).with_context(|| format!("read {}", key.display()))?;
        Issuer::new(
            &certificate_pem,
            &key_pem,
            load_certificates(fleet_server_ca)?,
            lifetime,
            tenant,
        )
        .with_context(|| {
            format!(
                "use {} and {} as the links' certificate authority, trusting {}",
                certificate.display(),
                key.display(),
                fleet_server_ca.display()
            )
        })
    }

    pub fn new(
        certificate_pem: &str,
        key_pem: &str,
        fleet_server_anchors: Vec<CertificateDer<'static>>,
        lifetime: Duration,
        tenant: Option<String>,
    ) -> anyhow::Result<Issuer> {
        let key_pair = KeyPair::from_pem(key_pem).context("parse the issuer's private key")?;
        let issuer = rcgen::Issuer::from_ca_cert_pem(certificate_pem, key_pair)
            .context("read the issuer's certificate with its key")?;
        let certificate = CertificateDer::pem_slice_iter(certificate_pem.as_bytes())
            .next()
            .context("the issuer's certificate is not PEM")?
            .context("parse the issuer's certificate")?;
        let mut roots = RootCertStore::empty();
        for anchor in fleet_server_anchors {
            roots.add(anchor).context("trust the fleet-server CA")?;
        }
        let verifier = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider())
            .build()
            .context("build the fleet-server certificate verifier")?;
        Ok(Issuer {
            certificate,
            issuer,
            verifier,
            lifetime,
            tenant,
        })
    }

    pub fn certificate(&self, device_id: &str, installation_id: &str) -> anyhow::Result<Minted> {
        let mut parameters = CertificateParams::default();
        parameters.distinguished_name = DistinguishedName::new();
        let mut uris = vec![
            format!("urn:dusk:device:{device_id}"),
            format!("urn:dusk:installation:{installation_id}"),
        ];
        if let Some(tenant) = &self.tenant {
            uris.push(format!("urn:dusk:tenant:{tenant}"));
        }
        for uri in uris {
            parameters
                .subject_alt_names
                .push(SanType::URI(uri.try_into()?));
        }
        let now = SystemTime::now();
        let not_after = now + self.lifetime;
        parameters.not_before = (now - BACKDATE).into();
        parameters.not_after = not_after.into();
        parameters.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        parameters.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let certificate = parameters.signed_by(&key, &self.issuer)?;
        Ok(Minted {
            chain: vec![certificate.der().clone(), self.certificate.clone()],
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            not_after,
        })
    }

    pub fn mint(&self, device_id: &str, installation_id: &str) -> anyhow::Result<Identity> {
        let minted = self.certificate(device_id, installation_id)?;
        let mut config = ClientConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_webpki_verifier(self.verifier.clone())
            .with_client_auth_cert(minted.chain, minted.key)?;
        config.resumption = Resumption::disabled();
        Ok(Identity {
            config: Arc::new(config),
            not_after: minted.not_after,
        })
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use rcgen::{BasicConstraints, IsCa};

    pub struct Authority {
        pub certificate: CertificateDer<'static>,
        pub certificate_pem: String,
        pub key_pem: String,
        issuer: rcgen::Issuer<'static, KeyPair>,
    }

    impl Authority {
        pub fn new(name: &str) -> Authority {
            let mut parameters = CertificateParams::default();
            parameters.distinguished_name = DistinguishedName::new();
            parameters
                .distinguished_name
                .push(rcgen::DnType::CommonName, name);
            parameters.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            parameters.key_usages = vec![KeyUsagePurpose::KeyCertSign];
            let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let certificate = parameters.self_signed(&key).unwrap();
            Authority {
                certificate: certificate.der().clone(),
                certificate_pem: certificate.pem(),
                key_pem: key.serialize_pem(),
                issuer: rcgen::Issuer::new(parameters, key),
            }
        }

        pub fn server(&self, name: &str) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
            let mut parameters = CertificateParams::new(vec![name.to_string()]).unwrap();
            parameters.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
            let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let certificate = parameters.signed_by(&key, &self.issuer).unwrap();
            (
                vec![certificate.der().clone()],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            )
        }

        pub fn issuer(&self, fleet_server: &Authority, lifetime: Duration) -> Issuer {
            Issuer::new(
                &self.certificate_pem,
                &self.key_pem,
                vec![fleet_server.certificate.clone()],
                lifetime,
                Some("acme".to_string()),
            )
            .unwrap()
        }
    }

    #[test]
    fn identities_are_dusk_ids_and_distinct_per_link() {
        let first = device_id(3, 7);
        let second = device_id(3, 8);
        for id in [
            &first,
            &second,
            &installation_id(&first),
            &installation_id(&second),
        ] {
            assert_eq!(id.len(), 32, "{id}");
            assert!(
                id.bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            );
        }
        assert_ne!(first, second);
        assert_ne!(installation_id(&first), installation_id(&second));
        assert_eq!(installation_id(&first), installation_id(&device_id(3, 7)));
    }

    #[test]
    fn a_certificate_is_renewed_an_hour_before_it_expires() {
        let fleet_client = Authority::new("fleet-client");
        let fleet_server = Authority::new("fleet-server");
        let device = device_id(0, 1);
        let lasting = fleet_client
            .issuer(&fleet_server, Duration::from_secs(7200))
            .mint(&device, &installation_id(&device))
            .unwrap();
        assert!(!lasting.renewal_due());
        let expiring = fleet_client
            .issuer(&fleet_server, RENEW_BEFORE - Duration::from_secs(60))
            .mint(&device, &installation_id(&device))
            .unwrap();
        assert!(expiring.renewal_due());
    }
}
