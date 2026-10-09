use rcgen::{
    BasicConstraints, CertificateParams, CertificateSigningRequestParams, DnType,
    ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose, SanType,
};
use rustls_pki_types::{
    CertificateDer, CertificateSigningRequestDer, PrivateKeyDer, PrivatePkcs8KeyDer,
};
use std::sync::Arc;
use std::time::Duration;

pub struct Authority {
    pub certificate: CertificateDer<'static>,
    pub pem: String,
    issuer: Issuer<'static, KeyPair>,
}

pub struct Issued {
    pub certificate: CertificateDer<'static>,
    pub key: PrivateKeyDer<'static>,
    pub certificate_pem: String,
    pub key_pem: String,
}

impl Issued {
    pub fn key(&self) -> PrivateKeyDer<'static> {
        self.key.clone_key()
    }
}

impl Authority {
    pub fn new(name: &str) -> Authority {
        let mut parameters = CertificateParams::new(Vec::<String>::new()).unwrap();
        parameters.distinguished_name.push(DnType::CommonName, name);
        parameters.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        parameters.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let certificate = parameters.self_signed(&key).unwrap();
        Authority {
            certificate: certificate.der().clone(),
            pem: certificate.pem(),
            issuer: Issuer::new(parameters, key),
        }
    }

    fn issue(
        &self,
        dns: &[&str],
        uris: &[String],
        usage: ExtendedKeyUsagePurpose,
        not_after: Option<time::OffsetDateTime>,
    ) -> Issued {
        let mut parameters =
            CertificateParams::new(dns.iter().map(|name| name.to_string()).collect::<Vec<_>>())
                .unwrap();
        if let Some(not_after) = not_after {
            parameters.not_before = time::OffsetDateTime::now_utc() - Duration::from_secs(60);
            parameters.not_after = not_after;
        }
        for uri in uris {
            parameters
                .subject_alt_names
                .push(SanType::URI(uri.as_str().try_into().unwrap()));
        }
        parameters.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        parameters.extended_key_usages = vec![usage];
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let certificate = parameters.signed_by(&key, &self.issuer).unwrap();
        Issued {
            certificate: certificate.der().clone(),
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            certificate_pem: certificate.pem(),
            key_pem: key.serialize_pem(),
        }
    }

    pub fn server(&self, dns: &[&str]) -> Issued {
        self.issue(dns, &[], ExtendedKeyUsagePurpose::ServerAuth, None)
    }

    pub fn node(&self, device_id: &str, installation_id: &str, tenant: Option<&str>) -> Issued {
        self.node_until(device_id, installation_id, tenant, None)
    }

    pub fn node_until(
        &self,
        device_id: &str,
        installation_id: &str,
        tenant: Option<&str>,
        not_after: Option<time::OffsetDateTime>,
    ) -> Issued {
        let mut uris = vec![
            format!("urn:dusk:device:{device_id}"),
            format!("urn:dusk:installation:{installation_id}"),
        ];
        if let Some(tenant) = tenant {
            uris.push(format!("urn:dusk:tenant:{tenant}"));
        }
        self.issue(&[], &uris, ExtendedKeyUsagePurpose::ClientAuth, not_after)
    }

    pub fn sign_request(
        &self,
        csr: &[u8],
        extra_uris: &[String],
        lifetime: Duration,
    ) -> CertificateDer<'static> {
        let mut request = CertificateSigningRequestParams::from_der(
            &CertificateSigningRequestDer::from(csr.to_vec()),
        )
        .unwrap();
        for uri in extra_uris {
            request
                .params
                .subject_alt_names
                .push(SanType::URI(uri.as_str().try_into().unwrap()));
        }
        let now = time::OffsetDateTime::now_utc();
        request.params.not_before = now - Duration::from_secs(60);
        request.params.not_after = now + lifetime;
        request.params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        request.params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        request.signed_by(&self.issuer).unwrap().der().clone()
    }

    pub fn principal(&self, name: &str) -> Issued {
        self.issue(
            &[],
            &[format!("urn:dusk:principal:{name}")],
            ExtendedKeyUsagePurpose::ClientAuth,
            None,
        )
    }
}

pub struct Pki {
    pub fleet_server: Authority,
    pub fleet_client: Arc<Authority>,
    pub internal: Authority,
}

impl Pki {
    pub fn new() -> Pki {
        Pki {
            fleet_server: Authority::new("fleet-server"),
            fleet_client: Arc::new(Authority::new("fleet-client")),
            internal: Authority::new("internal"),
        }
    }
}
