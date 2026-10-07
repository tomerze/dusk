use std::sync::{Arc, RwLock};
use std::time::Duration;

use rustls::client::danger::HandshakeSignatureValid;
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{CertificateError, DigitallySignedStruct, DistinguishedName, SignatureScheme};
use rustls_pki_types::{CertificateDer, TrustAnchor, UnixTime};
use x509_parser::extensions::{GeneralName, ParsedExtension};

use crate::identity::{CertificateIdentity, IdentityError, identity_from_uris};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenewCertificateError {
    #[error("no client certificate was presented")]
    Missing,
    #[error("the client certificate cannot be parsed")]
    Malformed,
    #[error("the client certificate does not chain to the fleet-client CA: {0}")]
    Untrusted(String),
    #[error("the client certificate is not valid yet")]
    NotYetValid,
    #[error(transparent)]
    Identity(#[from] IdentityError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenewCertificate {
    pub identity: CertificateIdentity,
    pub not_before_unix: i64,
    pub not_after_unix: i64,
    pub public_key: Vec<u8>,
}

#[derive(Debug)]
pub struct FleetClientTrust {
    anchors: RwLock<Arc<Vec<TrustAnchor<'static>>>>,
    algorithms: WebPkiSupportedAlgorithms,
}

fn trust_anchors(roots: &[CertificateDer<'_>]) -> Result<Vec<TrustAnchor<'static>>, String> {
    let anchors = roots
        .iter()
        .map(|root| {
            webpki::anchor_from_trusted_cert(root)
                .map(|anchor| anchor.to_owned())
                .map_err(|error| {
                    format!("a fleet-client root is not a usable trust anchor: {error}")
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if anchors.is_empty() {
        return Err(String::from("no fleet-client root is configured"));
    }
    Ok(anchors)
}

impl FleetClientTrust {
    pub fn new(roots: &[CertificateDer<'_>]) -> Result<FleetClientTrust, String> {
        Ok(FleetClientTrust {
            anchors: RwLock::new(Arc::new(trust_anchors(roots)?)),
            algorithms: rustls::crypto::ring::default_provider().signature_verification_algorithms,
        })
    }

    pub fn replace_roots(&self, roots: &[CertificateDer<'_>]) -> Result<(), String> {
        let anchors = Arc::new(trust_anchors(roots)?);
        *self
            .anchors
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = anchors;
        Ok(())
    }

    pub fn verify(
        &self,
        chain: &[CertificateDer<'_>],
        now_unix_seconds: u64,
    ) -> Result<RenewCertificate, RenewCertificateError> {
        let (leaf, intermediates) = chain.split_first().ok_or(RenewCertificateError::Missing)?;
        let (_, parsed) = x509_parser::parse_x509_certificate(leaf)
            .map_err(|_| RenewCertificateError::Malformed)?;
        let validity = parsed.validity();
        let not_before = validity.not_before.timestamp();
        let not_after = validity.not_after.timestamp();
        let now = i64::try_from(now_unix_seconds).unwrap_or(i64::MAX);
        if now < not_before {
            return Err(RenewCertificateError::NotYetValid);
        }
        let verification_time = if now > not_after {
            not_after.saturating_sub(1).max(not_before)
        } else {
            now
        };
        let time = UnixTime::since_unix_epoch(Duration::from_secs(
            u64::try_from(verification_time).unwrap_or(0),
        ));
        let end_entity = webpki::EndEntityCert::try_from(leaf)
            .map_err(|error| RenewCertificateError::Untrusted(error.to_string()))?;
        let anchors = self
            .anchors
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        end_entity
            .verify_for_usage(
                self.algorithms.all,
                &anchors,
                intermediates,
                time,
                webpki::KeyUsage::client_auth(),
                None,
                None,
            )
            .map_err(|error| RenewCertificateError::Untrusted(error.to_string()))?;
        let mut uris = Vec::new();
        for extension in parsed.extensions() {
            if let ParsedExtension::SubjectAlternativeName(names) = extension.parsed_extension() {
                for name in &names.general_names {
                    match name {
                        GeneralName::URI(uri) => uris.push(*uri),
                        _ => {
                            return Err(RenewCertificateError::Identity(
                                IdentityError::UnexpectedUri(format!("{name:?}")),
                            ));
                        }
                    }
                }
            }
        }
        Ok(RenewCertificate {
            identity: identity_from_uris(uris)?,
            not_before_unix: not_before,
            not_after_unix: not_after,
            public_key: parsed.public_key().raw.to_vec(),
        })
    }
}

#[derive(Debug)]
pub struct RenewClientVerifier {
    trust: Arc<FleetClientTrust>,
}

impl RenewClientVerifier {
    pub fn new(trust: Arc<FleetClientTrust>) -> Arc<RenewClientVerifier> {
        Arc::new(RenewClientVerifier { trust })
    }
}

impl ClientCertVerifier for RenewClientVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        false
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
        let mut chain = Vec::with_capacity(intermediates.len() + 1);
        chain.push(end_entity.clone());
        chain.extend(intermediates.iter().cloned());
        match self.trust.verify(&chain, now.as_secs()) {
            Ok(_) => Ok(ClientCertVerified::assertion()),
            Err(RenewCertificateError::NotYetValid) => Err(rustls::Error::InvalidCertificate(
                CertificateError::NotValidYet,
            )),
            Err(RenewCertificateError::Malformed) => Err(rustls::Error::InvalidCertificate(
                CertificateError::BadEncoding,
            )),
            Err(error) => Err(rustls::Error::InvalidCertificate(CertificateError::Other(
                rustls::OtherError(Arc::new(error)),
            ))),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            certificate,
            signature,
            &self.trust.algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            certificate,
            signature,
            &self.trust.algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.trust.algorithms.supported_schemes()
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer,
        KeyPair, KeyUsagePurpose, SanType,
    };
    use rustls_pki_types::CertificateDer;
    use time::OffsetDateTime;

    pub(crate) struct Authority {
        pub(crate) root: CertificateDer<'static>,
        pub(crate) intermediate: CertificateDer<'static>,
        issuer: Issuer<'static, KeyPair>,
    }

    impl Authority {
        pub(crate) fn new(name: &str) -> Authority {
            let root_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let mut root_params = CertificateParams::new(Vec::<String>::new()).unwrap();
            root_params
                .distinguished_name
                .push(DnType::CommonName, format!("{name} root"));
            root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            root_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
            let root = root_params.self_signed(&root_key).unwrap();
            let root_issuer = Issuer::new(root_params, root_key);
            let intermediate_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let mut intermediate_params = CertificateParams::new(Vec::<String>::new()).unwrap();
            intermediate_params
                .distinguished_name
                .push(DnType::CommonName, format!("{name} intermediate"));
            intermediate_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
            intermediate_params.key_usages =
                vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
            let intermediate = intermediate_params
                .signed_by(&intermediate_key, &root_issuer)
                .unwrap();
            Authority {
                root: root.der().clone(),
                intermediate: intermediate.der().clone(),
                issuer: Issuer::new(intermediate_params, intermediate_key),
            }
        }

        pub(crate) fn leaf(
            &self,
            key: &KeyPair,
            uris: &[String],
            not_before: OffsetDateTime,
            not_after: OffsetDateTime,
        ) -> CertificateDer<'static> {
            let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
            params.subject_alt_names = uris
                .iter()
                .map(|uri| SanType::URI(uri.as_str().try_into().unwrap()))
                .collect();
            params.not_before = not_before;
            params.not_after = not_after;
            params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
            params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
            params.signed_by(key, &self.issuer).unwrap().der().clone()
        }

        pub(crate) fn sign_request(
            &self,
            csr_der: &[u8],
            extra_uris: &[String],
            lifetime: time::Duration,
        ) -> CertificateDer<'static> {
            let mut request =
                rcgen::CertificateSigningRequestParams::from_der(&csr_der.to_vec().into()).unwrap();
            request.params.subject_alt_names.extend(
                extra_uris
                    .iter()
                    .map(|uri| SanType::URI(uri.as_str().try_into().unwrap())),
            );
            let now = OffsetDateTime::now_utc();
            request.params.not_before = now - time::Duration::minutes(1);
            request.params.not_after = now + lifetime;
            request.params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
            request.params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
            request.signed_by(&self.issuer).unwrap().der().clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;

    use super::fixtures::Authority;
    use super::*;
    use crate::csr::fixtures::p256;
    use crate::identity::{device_uri, installation_uri, tenant_uri};

    const DEVICE: &str = "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13";
    const INSTALLATION: &str = "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70";

    fn uris() -> Vec<String> {
        vec![
            device_uri(DEVICE),
            installation_uri(INSTALLATION),
            tenant_uri("retail-eu"),
        ]
    }

    #[test]
    fn accepts_a_current_and_an_expired_certificate_of_the_fleet_client_ca() {
        let authority = Authority::new("fleet-client");
        let trust = FleetClientTrust::new(std::slice::from_ref(&authority.root)).unwrap();
        let key = p256();
        let start = OffsetDateTime::from_unix_timestamp(1_790_000_000).unwrap();
        let end = start + time::Duration::hours(168);
        let leaf = authority.leaf(&key, &uris(), start, end);
        let chain = [leaf, authority.intermediate.clone()];
        let current = trust.verify(&chain, 1_790_100_000).unwrap();
        assert_eq!(current.identity.device_id, DEVICE);
        assert_eq!(current.identity.tenant.as_deref(), Some("retail-eu"));
        assert_eq!(current.not_after_unix, end.unix_timestamp());
        assert_eq!(
            current.public_key,
            rcgen::PublicKeyData::subject_public_key_info(&key)
        );
        let long_expired = end.unix_timestamp() as u64 + 400 * 24 * 3600;
        assert_eq!(
            trust.verify(&chain, long_expired).unwrap().identity,
            current.identity
        );
        assert_eq!(
            trust.verify(&chain, 1_780_000_000),
            Err(RenewCertificateError::NotYetValid)
        );
    }

    #[test]
    fn refuses_another_ca_a_missing_chain_and_foreign_names() {
        let authority = Authority::new("fleet-client");
        let stranger = Authority::new("internal");
        let trust = FleetClientTrust::new(std::slice::from_ref(&authority.root)).unwrap();
        let start = OffsetDateTime::now_utc() - time::Duration::hours(1);
        let end = start + time::Duration::hours(168);
        let now = OffsetDateTime::now_utc().unix_timestamp() as u64;
        let foreign = stranger.leaf(&p256(), &uris(), start, end);
        assert!(matches!(
            trust.verify(&[foreign, stranger.intermediate.clone()], now),
            Err(RenewCertificateError::Untrusted(_))
        ));
        let leaf = authority.leaf(&p256(), &uris(), start, end);
        assert!(matches!(
            trust.verify(std::slice::from_ref(&leaf), now),
            Err(RenewCertificateError::Untrusted(_))
        ));
        assert_eq!(trust.verify(&[], now), Err(RenewCertificateError::Missing));
        let principal = authority.leaf(
            &p256(),
            &[String::from("urn:dusk:principal:dawn-0")],
            start,
            end,
        );
        assert!(matches!(
            trust.verify(&[principal, authority.intermediate.clone()], now),
            Err(RenewCertificateError::Identity(_))
        ));
    }

    #[test]
    fn refuses_a_root_once_it_is_replaced() {
        let retired = Authority::new("retired fleet-client");
        let current = Authority::new("fleet-client");
        let trust = FleetClientTrust::new(std::slice::from_ref(&retired.root)).unwrap();
        let start = OffsetDateTime::now_utc() - time::Duration::hours(1);
        let end = start + time::Duration::hours(168);
        let now = OffsetDateTime::now_utc().unix_timestamp() as u64;
        let chain = [
            retired.leaf(&p256(), &uris(), start, end),
            retired.intermediate.clone(),
        ];
        trust.verify(&chain, now).unwrap();
        trust
            .replace_roots(std::slice::from_ref(&current.root))
            .unwrap();
        assert!(matches!(
            trust.verify(&chain, now),
            Err(RenewCertificateError::Untrusted(_))
        ));
        trust
            .verify(
                &[
                    current.leaf(&p256(), &uris(), start, end),
                    current.intermediate.clone(),
                ],
                now,
            )
            .unwrap();
        assert!(trust.replace_roots(&[]).is_err());
        trust
            .verify(
                &[
                    current.leaf(&p256(), &uris(), start, end),
                    current.intermediate.clone(),
                ],
                now,
            )
            .unwrap();
    }

    #[test]
    fn answers_rustls_for_the_provision_listener() {
        let authority = Authority::new("fleet-client");
        let trust = Arc::new(FleetClientTrust::new(std::slice::from_ref(&authority.root)).unwrap());
        let verifier = RenewClientVerifier::new(trust);
        assert!(verifier.offer_client_auth());
        assert!(!verifier.client_auth_mandatory());
        assert!(verifier.root_hint_subjects().is_empty());
        let start = OffsetDateTime::from_unix_timestamp(1_790_000_000).unwrap();
        let leaf = authority.leaf(&p256(), &uris(), start, start + time::Duration::hours(168));
        let much_later = UnixTime::since_unix_epoch(Duration::from_secs(1_900_000_000));
        verifier
            .verify_client_cert(
                &leaf,
                std::slice::from_ref(&authority.intermediate),
                much_later,
            )
            .unwrap();
        let stranger = Authority::new("other");
        assert!(
            verifier
                .verify_client_cert(
                    &leaf,
                    std::slice::from_ref(&stranger.intermediate),
                    much_later
                )
                .is_err()
        );
    }
}
