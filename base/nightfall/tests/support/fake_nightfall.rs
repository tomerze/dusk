#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aes::cipher::{BlockEncrypt as _, KeyInit as _, generic_array::GenericArray};
use async_io::{Async, Timer};
use capnp::capability::{FromClientHook as _, Promise};
use dusk_capnp::capnp_rpc::{self, RpcSystem, rpc_twoparty_capnp::Side, twoparty};
use dusk_capnp::dusk_capnp::dusk;
use dusk_core::driver::DuskImplExit;
use dusk_program::value::Value;
use dusk_program_nightfall::provision_capnp::{self, provisioning};
use futures::AsyncReadExt as _;
use futures::task::LocalSpawnExt as _;
use futures_rustls::TlsAcceptor;
use ring::signature::KeyPair as _;
use rustls::client::danger::HandshakeSignatureValid;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};
use tpm2_protocol::TpmUnmarshal as _;
use tpm2_protocol::data::{TpmaObject, TpmtPublic, TpmuPublicId};
use x509_parser::extensions::{GeneralName, ParsedExtension};
use x509_parser::prelude::FromDer as _;

const DEVICE_ID: &str = "0123456789abcdef0123456789abcdef";
pub(crate) const INSTALL_TOKEN: &str = "an-install-token-for-the-tests";
pub(crate) const TPM_BOUND_URI: &str = "urn:dusk:attestation:tpm";
pub(crate) const FIRST_LINK_TIMEOUT: Duration = Duration::from_secs(40);
pub(crate) const INSTALLATION_ID: u64 =
    dusk_program_kvs_internal::key_id("nightfall.installation_id");
pub(crate) const PRIVATE_KEY: u64 = dusk_program_kvs_internal::key_id("nightfall.private_key");
pub(crate) const STAGED_PRIVATE_KEY: u64 =
    dusk_program_kvs_internal::key_id("nightfall.staged_private_key");
pub(crate) const CERTIFICATE_CHAIN: u64 =
    dusk_program_kvs_internal::key_id("nightfall.certificate_chain");
pub(crate) const HARDWARE_FINGERPRINT: u64 =
    dusk_program_kvs_internal::key_id("nightfall.hardware_fingerprint");
const DEVICE_ID_KEY: u64 = dusk_program_kvs_internal::key_id("dusk.device.id");
pub(crate) const KEPT: u8 =
    dusk_program_kvs_internal::FLAG_STICKY | dusk_program_kvs_internal::FLAG_PERSISTENT;
pub(crate) const KEPT_SECRET: u8 = KEPT | dusk_program_kvs_internal::FLAG_SENSITIVE;

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub(crate) fn now_seconds() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}

fn time_from_unix(seconds: i64) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(seconds).unwrap()
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn fingerprint(der: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, der).as_ref())
}

fn random_bytes<const LENGTH: usize>() -> [u8; LENGTH] {
    let mut bytes = [0u8; LENGTH];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes).unwrap();
    bytes
}

fn random_identifier() -> String {
    hex(&random_bytes::<16>())
}

pub(crate) fn sha256(bytes: &[u8]) -> Vec<u8> {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .to_vec()
}

fn derive_key(key: &[u8], label: &str, context: &[u8], bits: u32) -> Vec<u8> {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    let length = usize::try_from(bits.div_ceil(8)).unwrap();
    let mut output = Vec::new();
    for counter in 1u32.. {
        if output.len() >= length {
            break;
        }
        let mut hmac = ring::hmac::Context::with_key(&key);
        hmac.update(&counter.to_be_bytes());
        hmac.update(label.as_bytes());
        hmac.update(&[0]);
        hmac.update(context);
        hmac.update(&bits.to_be_bytes());
        output.extend_from_slice(hmac.sign().as_ref());
    }
    output.truncate(length);
    output
}

fn mask(seed: &[u8], length: usize) -> Vec<u8> {
    let mut output = Vec::new();
    for counter in 0u32.. {
        if output.len() >= length {
            break;
        }
        output.extend_from_slice(&sha256(&[seed, &counter.to_be_bytes()].concat()));
    }
    output.truncate(length);
    output
}

fn rsa_oaep_sha256(modulus: &[u8], label: &[u8], message: &[u8]) -> Vec<u8> {
    let size = modulus.len();
    let mut block = sha256(label);
    block.resize(size - message.len() - 34, 0);
    block.push(0x01);
    block.extend_from_slice(message);
    let mut seed = random_bytes::<32>().to_vec();
    for (byte, masked) in block.iter_mut().zip(mask(&seed, size - 33)) {
        *byte ^= masked;
    }
    for (byte, masked) in seed.iter_mut().zip(mask(&block, 32)) {
        *byte ^= masked;
    }
    let encoded = [&[0u8][..], &seed, &block].concat();
    let encrypted = num_bigint::BigUint::from_bytes_be(&encoded)
        .modpow(
            &num_bigint::BigUint::from(65537u32),
            &num_bigint::BigUint::from_bytes_be(modulus),
        )
        .to_bytes_be();
    let mut padded = vec![0u8; size - encrypted.len()];
    padded.extend_from_slice(&encrypted);
    padded
}

fn aes_128_cfb(key: &[u8], data: &mut [u8]) {
    let cipher = aes::Aes128::new(GenericArray::from_slice(key));
    let mut feedback = [0u8; 16];
    for chunk in data.chunks_mut(16) {
        let mut stream = GenericArray::clone_from_slice(&feedback);
        cipher.encrypt_block(&mut stream);
        for (byte, key) in chunk.iter_mut().zip(stream.iter()) {
            *byte ^= key;
        }
        feedback[..chunk.len()].copy_from_slice(chunk);
    }
}

pub(crate) fn make_credential(
    endorsement_modulus: &[u8],
    name: &[u8],
    secret: &[u8],
) -> (Vec<u8>, Vec<u8>) {
    let seed = random_bytes::<32>();
    let encrypted_secret = rsa_oaep_sha256(endorsement_modulus, b"IDENTITY\0", &seed);
    let mut identity = u16::try_from(secret.len()).unwrap().to_be_bytes().to_vec();
    identity.extend_from_slice(secret);
    aes_128_cfb(&derive_key(&seed, "STORAGE", name, 128), &mut identity);
    let integrity = ring::hmac::Key::new(
        ring::hmac::HMAC_SHA256,
        &derive_key(&seed, "INTEGRITY", &[], 256),
    );
    let mut hmac = ring::hmac::Context::with_key(&integrity);
    hmac.update(&identity);
    hmac.update(name);
    let mut credential_blob = 32u16.to_be_bytes().to_vec();
    credential_blob.extend_from_slice(hmac.sign().as_ref());
    credential_blob.extend_from_slice(&identity);
    (credential_blob, encrypted_secret)
}

fn strip_zeros(bytes: &[u8]) -> &[u8] {
    &bytes[bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len())..]
}

fn parse_public(public: &[u8]) -> Result<TpmtPublic, String> {
    match TpmtPublic::unmarshal(public) {
        Ok((parsed, [])) => Ok(parsed),
        Ok(_) => Err(String::from("trailing bytes after a TPM public area")),
        Err(error) => Err(format!("not a TPM public area: {error}")),
    }
}

struct Attested {
    endorsement_modulus: Vec<u8>,
    node_key_name: Vec<u8>,
    node_key_point: Vec<u8>,
}

fn attest(report: &Report, tpm: &TpmAttestation) -> Result<Attested, String> {
    let endorsement = parse_public(&tpm.endorsement_key)?;
    let TpmuPublicId::Rsa(modulus) = &endorsement.unique else {
        return Err(String::from("the endorsement key is not RSA"));
    };
    if report.hardware_fingerprint != sha256(&tpm.endorsement_key) {
        return Err(String::from(
            "the hardware fingerprint is not the SHA-256 of the endorsement key",
        ));
    }
    let (_, certificate) =
        x509_parser::certificate::X509Certificate::from_der(&tpm.endorsement_certificate)
            .map_err(|error| format!("the endorsement certificate is unreadable: {error}"))?;
    let x509_parser::public_key::PublicKey::RSA(certified) = certificate
        .public_key()
        .parsed()
        .map_err(|error| format!("the endorsement certificate's key is unreadable: {error}"))?
    else {
        return Err(String::from(
            "the endorsement certificate does not certify an RSA key",
        ));
    };
    if strip_zeros(certified.modulus) != strip_zeros(modulus)
        || strip_zeros(certified.exponent) != [0x01, 0x00, 0x01]
    {
        return Err(String::from(
            "the endorsement certificate certifies another key",
        ));
    }
    let node_key = parse_public(&tpm.node_key)?;
    let required = TpmaObject::FIXED_TPM
        | TpmaObject::FIXED_PARENT
        | TpmaObject::SENSITIVE_DATA_ORIGIN
        | TpmaObject::SIGN_ENCRYPT;
    if !node_key.object_attributes.contains(required)
        || node_key.object_attributes & (TpmaObject::RESTRICTED | TpmaObject::DECRYPT)
            != TpmaObject::empty()
    {
        return Err(format!(
            "the node key's attributes {:?} do not make it a TPM-resident signing key",
            node_key.object_attributes
        ));
    }
    let TpmuPublicId::Ecc(point) = &node_key.unique else {
        return Err(String::from("the node key is not ECC"));
    };
    let mut node_key_point = vec![0x04];
    node_key_point.extend_from_slice(&point.x);
    node_key_point.extend_from_slice(&point.y);
    let mut node_key_name = vec![0x00, 0x0b];
    node_key_name.extend_from_slice(&sha256(&tpm.node_key));
    Ok(Attested {
        endorsement_modulus: modulus.to_vec(),
        node_key_name,
        node_key_point,
    })
}

pub(crate) fn public_key_of(pkcs8: &[u8]) -> Vec<u8> {
    ring::signature::EcdsaKeyPair::from_pkcs8(
        &ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
        pkcs8,
        &ring::rand::SystemRandom::new(),
    )
    .unwrap()
    .public_key()
    .as_ref()
    .to_vec()
}

struct RawPublicKey(Vec<u8>);

impl rcgen::PublicKeyData for RawPublicKey {
    fn der_bytes(&self) -> &[u8] {
        &self.0
    }

    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        &rcgen::PKCS_ECDSA_P256_SHA256
    }
}

pub(crate) fn authority(
    common_name: &str,
) -> (rcgen::Issuer<'static, rcgen::KeyPair>, rcgen::Certificate) {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut parameters = rcgen::CertificateParams::default();
    parameters.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    parameters
        .distinguished_name
        .push(rcgen::DnType::CommonName, common_name);
    parameters.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    let certificate = parameters.self_signed(&key).unwrap();
    (rcgen::Issuer::new(parameters, key), certificate)
}

struct Authorities {
    server_authority_pem: String,
    server_chain: Vec<CertificateDer<'static>>,
    server_key: Vec<u8>,
    client_authority: rcgen::Issuer<'static, rcgen::KeyPair>,
    client_authority_der: CertificateDer<'static>,
}

impl Authorities {
    fn new() -> Authorities {
        let (server_authority, server_authority_certificate) =
            authority("fleet-server CA for tests");
        let (client_authority, client_authority_certificate) =
            authority("fleet-client CA for tests");
        let server_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let parameters = rcgen::CertificateParams::new(vec![
            String::from("fleet.test"),
            String::from("provision.test"),
        ])
        .unwrap();
        let server_certificate = parameters
            .signed_by(&server_key, &server_authority)
            .unwrap();
        Authorities {
            server_authority_pem: server_authority_certificate.pem(),
            server_chain: vec![server_certificate.der().clone()],
            server_key: server_key.serialize_der(),
            client_authority,
            client_authority_der: client_authority_certificate.der().clone(),
        }
    }

    fn issue_node_certificate(
        &self,
        installation_id: &str,
        public_key: &[u8],
        validity_seconds: (i64, i64),
        tpm_bound: bool,
    ) -> CertificateDer<'static> {
        let mut parameters = rcgen::CertificateParams::default();
        parameters.distinguished_name = rcgen::DistinguishedName::new();
        parameters.subject_alt_names = vec![
            rcgen::SanType::URI(
                rcgen::string::Ia5String::try_from(format!("urn:dusk:device:{DEVICE_ID}")).unwrap(),
            ),
            rcgen::SanType::URI(
                rcgen::string::Ia5String::try_from(format!(
                    "urn:dusk:installation:{installation_id}"
                ))
                .unwrap(),
            ),
        ];
        if tpm_bound {
            parameters.subject_alt_names.push(rcgen::SanType::URI(
                rcgen::string::Ia5String::try_from(TPM_BOUND_URI).unwrap(),
            ));
        }
        parameters.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
        parameters.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
        parameters.not_before = time_from_unix(validity_seconds.0);
        parameters.not_after = time_from_unix(validity_seconds.1);
        parameters
            .signed_by(&RawPublicKey(public_key.to_vec()), &self.client_authority)
            .unwrap()
            .der()
            .clone()
    }

    fn server_config(
        &self,
        client_verifier: Arc<dyn ClientCertVerifier>,
        versions: &[&'static rustls::SupportedProtocolVersion],
    ) -> Arc<rustls::ServerConfig> {
        let config = rustls::ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(versions)
            .unwrap()
            .with_client_cert_verifier(client_verifier)
            .with_single_cert(
                self.server_chain.clone(),
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.server_key.clone())),
            )
            .unwrap();
        Arc::new(config)
    }
}

#[derive(Debug)]
struct RecordAnyClientCertificate(Arc<CryptoProvider>);

impl ClientCertVerifier for RecordAnyClientCertificate {
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
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
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
            &self.0.signature_verification_algorithms,
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
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Report {
    pub(crate) hardware_fingerprint: Vec<u8>,
    pub(crate) installation_hint: String,
    pub(crate) dusk_version: String,
    pub(crate) impl_name: String,
    pub(crate) target_os: String,
    pub(crate) target_arch: String,
    pub(crate) hostname: String,
    pub(crate) tpm: Option<Box<TpmAttestation>>,
}

#[derive(Debug, Clone)]
pub(crate) struct TpmAttestation {
    pub(crate) endorsement_key: Vec<u8>,
    pub(crate) endorsement_certificate: Vec<u8>,
    pub(crate) endorsement_certificate_chain: Vec<Vec<u8>>,
    pub(crate) node_key: Vec<u8>,
}

#[derive(Debug, Clone)]
pub(crate) struct Stored {
    pub(crate) installation_id: Option<(Value, u8)>,
    pub(crate) private_key: Option<(Value, u8)>,
    pub(crate) staged_private_key: Option<(Value, u8)>,
    pub(crate) certificate_chain: Option<(Value, u8)>,
    pub(crate) hardware_fingerprint: Option<(Value, u8)>,
    pub(crate) device_id: Option<(Value, u8)>,
}

impl Stored {
    pub(crate) fn certificate(&self) -> String {
        let Some((Value::List(chain), _)) = &self.certificate_chain else {
            panic!("no certificate chain is stored: {self:?}");
        };
        let Some(Value::Bytes(leaf)) = chain.first() else {
            panic!("the stored chain holds no certificate: {self:?}");
        };
        fingerprint(leaf)
    }

    pub(crate) fn public_key(&self) -> Vec<u8> {
        let Some((Value::Bytes(pkcs8), _)) = &self.private_key else {
            panic!("no private key is stored: {self:?}");
        };
        public_key_of(pkcs8)
    }

    pub(crate) fn installation(&self) -> String {
        let Some((Value::String(installation_id), _)) = &self.installation_id else {
            panic!("no installation id is stored: {self:?}");
        };
        installation_id.clone()
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    AssignAttempted,
    Assigned {
        installation_id: String,
        report: Report,
        credential: &'static str,
        server_name: Option<String>,
    },
    Enrolled {
        installation_id: String,
        certificate: String,
        public_key: Vec<u8>,
        tpm_bound: bool,
    },
    Renewed {
        presented: String,
        certificate: String,
        public_key: Vec<u8>,
        tpm_bound: bool,
    },
    RenewRefused {
        presented: String,
    },
    Linked {
        connection: usize,
        certificate: String,
        namespace_id: u64,
        hostname: String,
        server_name: Option<String>,
    },
    LinkClosed {
        connection: usize,
        lived: Duration,
    },
    Stored {
        connection: usize,
        stored: Box<Stored>,
    },
    Replaced {
        certificate: String,
    },
    Stopped,
    Accepted {
        connection: usize,
    },
    HandshakeFailed {
        listener: &'static str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkMode {
    Serve,
    DropAfterLinked,
    StoredThenServe,
    StoredThenStopNode,
    ReplaceCertificateThenStopNode { validity_seconds: (i64, i64) },
    Silent,
    NoHandshake,
}

#[derive(Clone)]
pub(crate) struct Behaviour {
    pub(crate) certificate_lifetime_seconds: i64,
    pub(crate) refuse_renew_as_beyond_grace: bool,
    pub(crate) fleet_offers_only_tls12: bool,
    pub(crate) attest_tpm: bool,
    pub(crate) credential_for_another_key: bool,
    pub(crate) links: Vec<LinkMode>,
}

impl Default for Behaviour {
    fn default() -> Self {
        Behaviour {
            certificate_lifetime_seconds: 3600,
            refuse_renew_as_beyond_grace: false,
            fleet_offers_only_tls12: false,
            attest_tpm: false,
            credential_for_another_key: false,
            links: Vec::new(),
        }
    }
}

struct Binding {
    challenge: Vec<u8>,
    node_key_point: Option<Vec<u8>>,
}

struct ProvisioningState {
    authorities: Arc<Authorities>,
    behaviour: Behaviour,
    events: Sender<Event>,
    challenges: RefCell<HashMap<String, Binding>>,
}

struct FakeProvisioning {
    state: Rc<ProvisioningState>,
    presented: Option<CertificateDer<'static>>,
    server_name: Option<String>,
}

struct CertificateRequest {
    public_key: Vec<u8>,
    installation_id: String,
}

fn read_certificate_request(der: &[u8]) -> Result<CertificateRequest, String> {
    let (remainder, request) =
        x509_parser::certification_request::X509CertificationRequest::from_der(der)
            .map_err(|error| format!("not a PKCS#10 request: {error}"))?;
    if !remainder.is_empty() {
        return Err(String::from("trailing bytes after the request"));
    }
    let information = &request.certification_request_info;
    let public_key = information.subject_pki.subject_public_key.data.to_vec();
    ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_ASN1, &public_key)
        .verify(information.raw, request.signature_value.data.as_ref())
        .map_err(|_| String::from("the request's self-signature does not verify"))?;
    if information.subject.iter().count() != 0 {
        return Err(format!(
            "the request's subject is not empty: {}",
            information.subject
        ));
    }
    let extensions: Vec<&ParsedExtension> = request
        .requested_extensions()
        .ok_or_else(|| String::from("the request carries no extensions"))?
        .collect();
    let [ParsedExtension::SubjectAlternativeName(names)] = extensions.as_slice() else {
        return Err(format!(
            "expected exactly a SAN extension, got {extensions:?}"
        ));
    };
    let uris: Vec<&str> = names
        .general_names
        .iter()
        .map(|name| match name {
            GeneralName::URI(uri) => Ok(*uri),
            other => Err(format!("a SAN that is not a URI: {other}")),
        })
        .collect::<Result<_, _>>()?;
    let [device, installation] = uris.as_slice() else {
        return Err(format!("expected exactly two SAN URIs, got {uris:?}"));
    };
    if *device != format!("urn:dusk:device:{DEVICE_ID}") {
        return Err(format!("unexpected device URI {device}"));
    }
    let installation_id = installation
        .strip_prefix("urn:dusk:installation:")
        .ok_or_else(|| format!("unexpected installation URI {installation}"))?
        .to_string();
    Ok(CertificateRequest {
        public_key,
        installation_id,
    })
}

impl FakeProvisioning {
    fn issue(
        &self,
        installation_id: &str,
        public_key: &[u8],
        tpm_bound: bool,
        mut issued: provision_capnp::issued::Builder<'_>,
    ) -> String {
        let now = now_seconds();
        let not_after = now - 1 + self.state.behaviour.certificate_lifetime_seconds;
        let leaf = self.state.authorities.issue_node_certificate(
            installation_id,
            public_key,
            (now - 1, not_after),
            tpm_bound,
        );
        let mut chain = issued.reborrow().init_certificate_chain(2);
        chain.set(0, leaf.as_ref());
        chain.set(1, self.state.authorities.client_authority_der.as_ref());
        issued.set_not_after_unix_ms(u64::try_from(not_after).unwrap() * 1000);
        fingerprint(&leaf)
    }
}

fn text(reader: capnp::Result<capnp::text::Reader<'_>>) -> capnp::Result<String> {
    reader?
        .to_string()
        .map_err(|error| capnp::Error::failed(error.to_string()))
}

impl provisioning::Server for FakeProvisioning {
    fn assign(
        &mut self,
        params: provisioning::AssignParams,
        mut results: provisioning::AssignResults,
    ) -> Promise<(), capnp::Error> {
        self.state.events.send(Event::AssignAttempted).ok();
        let outcome = (|| {
            let params = params.get()?;
            let credential = match params.get_credential()?.which()? {
                provision_capnp::credential::Which::FleetToken(token) => {
                    if text(token)? != dusk_core::fleet_token::fleet_token() {
                        return Err(capnp::Error::failed(String::from(
                            "denied: unknown fleet token",
                        )));
                    }
                    "fleet"
                }
                provision_capnp::credential::Which::InstallToken(token) => {
                    if text(token)? != INSTALL_TOKEN {
                        return Err(capnp::Error::failed(String::from(
                            "denied: unknown install token",
                        )));
                    }
                    "install"
                }
                _ => {
                    return Err(capnp::Error::failed(String::from("denied: not a token")));
                }
            };
            let installation_id = random_identifier();
            let device = params.get_device()?;
            let report = Report {
                hardware_fingerprint: device.get_hardware_fingerprint()?.to_vec(),
                installation_hint: text(device.get_installation_hint())?,
                dusk_version: text(device.get_dusk_version())?,
                impl_name: text(device.get_impl())?,
                target_os: text(device.get_target_os())?,
                target_arch: text(device.get_target_arch())?,
                hostname: text(device.get_hostname())?,
                tpm: if device.has_tpm() {
                    let tpm = device.get_tpm()?;
                    Some(Box::new(TpmAttestation {
                        endorsement_key: tpm.get_endorsement_key()?.to_vec(),
                        endorsement_certificate: tpm.get_endorsement_certificate()?.to_vec(),
                        endorsement_certificate_chain: tpm
                            .get_endorsement_certificate_chain()?
                            .iter()
                            .map(|certificate| certificate.map(<[u8]>::to_vec))
                            .collect::<capnp::Result<_>>()?,
                        node_key: tpm.get_node_key()?.to_vec(),
                    }))
                } else {
                    None
                },
            };
            let challenge = random_bytes::<32>().to_vec();
            let mut assignment = results.get().init_assignment();
            assignment.set_device_id(DEVICE_ID);
            assignment.set_installation_id(&installation_id);
            let node_key_point = match &report.tpm {
                Some(tpm) if self.state.behaviour.attest_tpm => {
                    let attested = attest(&report, tpm)
                        .map_err(|reason| capnp::Error::failed(format!("denied: {reason}")))?;
                    let name = if self.state.behaviour.credential_for_another_key {
                        [&[0x00, 0x0b][..], &random_bytes::<32>()].concat()
                    } else {
                        attested.node_key_name
                    };
                    let (credential_blob, encrypted_secret) =
                        make_credential(&attested.endorsement_modulus, &name, &challenge);
                    assignment.set_credential_blob(&credential_blob);
                    assignment.set_encrypted_secret(&encrypted_secret);
                    Some(attested.node_key_point)
                }
                _ => {
                    assignment.set_challenge(&challenge);
                    None
                }
            };
            self.state.challenges.borrow_mut().insert(
                installation_id.clone(),
                Binding {
                    challenge,
                    node_key_point,
                },
            );
            self.state
                .events
                .send(Event::Assigned {
                    installation_id,
                    report,
                    credential,
                    server_name: self.server_name.clone(),
                })
                .ok();
            Ok(())
        })();
        Promise::from_future(async move { outcome })
    }

    fn enroll(
        &mut self,
        params: provisioning::EnrollParams,
        mut results: provisioning::EnrollResults,
    ) -> Promise<(), capnp::Error> {
        let outcome = (|| {
            let params = params.get()?;
            let request =
                read_certificate_request(params.get_csr()?).map_err(capnp::Error::failed)?;
            let binding = self
                .state
                .challenges
                .borrow_mut()
                .remove(&request.installation_id)
                .ok_or_else(|| capnp::Error::failed(String::from("denied: no assignment")))?;
            if params.get_challenge()? != binding.challenge.as_slice() {
                return Err(capnp::Error::failed(String::from(
                    "denied: wrong challenge",
                )));
            }
            if binding
                .node_key_point
                .as_ref()
                .is_some_and(|point| *point != request.public_key)
            {
                return Err(capnp::Error::failed(String::from(
                    "denied: the request's key is not the attested node key",
                )));
            }
            let tpm_bound = binding.node_key_point.is_some();
            let certificate = self.issue(
                &request.installation_id,
                &request.public_key,
                tpm_bound,
                results.get().init_issued(),
            );
            self.state
                .events
                .send(Event::Enrolled {
                    installation_id: request.installation_id,
                    certificate,
                    public_key: request.public_key,
                    tpm_bound,
                })
                .ok();
            Ok(())
        })();
        Promise::from_future(async move { outcome })
    }

    fn renew(
        &mut self,
        params: provisioning::RenewParams,
        mut results: provisioning::RenewResults,
    ) -> Promise<(), capnp::Error> {
        let outcome = (|| {
            let presented = self.presented.clone().ok_or_else(|| {
                capnp::Error::failed(String::from("denied: renew needs the current certificate"))
            })?;
            let presented_fingerprint = fingerprint(&presented);
            if self.state.behaviour.refuse_renew_as_beyond_grace {
                self.state
                    .events
                    .send(Event::RenewRefused {
                        presented: presented_fingerprint,
                    })
                    .ok();
                return Err(capnp::Error::failed(String::from(
                    provision_capnp::RENEW_BEYOND_GRACE,
                )));
            }
            let request =
                read_certificate_request(params.get()?.get_csr()?).map_err(capnp::Error::failed)?;
            let (_, presented_certificate) =
                x509_parser::certificate::X509Certificate::from_der(&presented)
                    .map_err(|error| capnp::Error::failed(error.to_string()))?;
            let tpm_bound = presented_certificate
                .subject_alternative_name()
                .ok()
                .flatten()
                .is_some_and(|names| {
                    names
                        .value
                        .general_names
                        .contains(&GeneralName::URI(TPM_BOUND_URI))
                });
            let same_key =
                presented_certificate.public_key().subject_public_key.data == request.public_key;
            if tpm_bound != same_key {
                return Err(capnp::Error::failed(String::from(if tpm_bound {
                    "denied: a TPM-bound certificate renews with its own key"
                } else {
                    "denied: renew needs a new key"
                })));
            }
            let certificate = self.issue(
                &request.installation_id,
                &request.public_key,
                tpm_bound,
                results.get().init_issued(),
            );
            self.state
                .events
                .send(Event::Renewed {
                    presented: presented_fingerprint,
                    certificate,
                    public_key: request.public_key,
                    tpm_bound,
                })
                .ok();
            Ok(())
        })();
        Promise::from_future(async move { outcome })
    }
}

async fn serve_provisioning(
    stream: Async<TcpStream>,
    acceptor: TlsAcceptor,
    state: Rc<ProvisioningState>,
) {
    let stream = match acceptor.accept(stream).await {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("the fake provisioning server refused a handshake: {error}");
            state
                .events
                .send(Event::HandshakeFailed {
                    listener: "provision",
                })
                .ok();
            return;
        }
    };
    let connection = stream.get_ref().1;
    let presented = connection
        .peer_certificates()
        .and_then(|chain| chain.first())
        .map(|certificate| certificate.clone().into_owned());
    let server_name = connection.server_name().map(String::from);
    let (reader, writer) = stream.split();
    let network = twoparty::VatNetwork::new(reader, writer, Side::Server, Default::default());
    let server: provisioning::Client = capnp_rpc::new_client(FakeProvisioning {
        state,
        presented,
        server_name,
    });
    if let Err(error) = RpcSystem::new(Box::new(network), Some(server.client)).await {
        eprintln!("a provisioning connection ended with {error}");
    }
}

async fn stop_node(node: &dusk::Client) -> capnp::Result<()> {
    let listing = node.ps_request().send().promise.await?;
    for entry in listing.get()?.get_process_entries()?.iter() {
        let process = entry.get_process()?;
        let name = process.name_request().send().promise.await?;
        if text(name.get()?.get_result())? == "init" {
            let mut kill = node.kill_request();
            kill.get().set_pid(entry.get_pid());
            kill.get().set_signal(15);
            if let Err(error) = kill.send().promise.await {
                eprintln!("killing init ended with {error}");
            }
            return Ok(());
        }
    }
    Err(capnp::Error::failed(String::from("no init process")))
}

type KvsPortal = dusk_base::dusk_program_kvs::kvs_capnp::kvs_portal::Client;

async fn with_kvs<Output>(
    node: &dusk::Client,
    use_kvs: impl AsyncFnOnce(&KvsPortal) -> capnp::Result<Output>,
) -> capnp::Result<Output> {
    let program_args = dusk_base::dusk_program_kvs::Args::bind().as_program_args()?;
    let mut process_request = node.process_request();
    program_args.with_reader(|reader| process_request.get().set_program_args(reader))?;
    let process = process_request.send().promise.await?.get()?.get_result()?;
    let pid = process
        .pid_request()
        .send()
        .promise
        .await?
        .get()?
        .get_result();
    let mut run = node.run_request();
    run.get().set_process(process.clone());
    run.send().promise.await?;
    let portal: KvsPortal = process
        .portal_request()
        .send()
        .promise
        .await?
        .get()?
        .get_result()?
        .cast_to();
    let output = use_kvs(&portal).await;
    let mut kill = node.kill_request();
    kill.get().set_pid(pid);
    kill.get().set_signal(15);
    kill.send().promise.await?;
    let mut waitpid = node.waitpid_request();
    waitpid.get().set_pid(pid);
    waitpid.send().promise.await?;
    output
}

async fn read_key(kvs: &KvsPortal, key: u64) -> capnp::Result<Option<(Value, u8)>> {
    let mut exists = kvs.exists_request();
    exists.get().set_key(key);
    if !exists.send().promise.await?.get()?.get_exists() {
        return Ok(None);
    }
    let mut get = kvs.get_request();
    get.get().set_key(key);
    let response = get.send().promise.await?;
    let response = response.get()?;
    Ok(Some((
        Value::from_reader(response.get_value()?)?,
        response.get_flags(),
    )))
}

async fn read_stored(kvs: &KvsPortal) -> capnp::Result<Stored> {
    Ok(Stored {
        installation_id: read_key(kvs, INSTALLATION_ID).await?,
        private_key: read_key(kvs, PRIVATE_KEY).await?,
        staged_private_key: read_key(kvs, STAGED_PRIVATE_KEY).await?,
        certificate_chain: read_key(kvs, CERTIFICATE_CHAIN).await?,
        hardware_fingerprint: read_key(kvs, HARDWARE_FINGERPRINT).await?,
        device_id: read_key(kvs, DEVICE_ID_KEY).await?,
    })
}

async fn replace_certificate(
    node: &dusk::Client,
    authorities: &Authorities,
    validity_seconds: (i64, i64),
) -> capnp::Result<String> {
    with_kvs(node, async |kvs: &KvsPortal| {
        let stored = read_stored(kvs).await?;
        let certificate = authorities.issue_node_certificate(
            &stored.installation(),
            &stored.public_key(),
            validity_seconds,
            false,
        );
        let mut set = kvs.set_request();
        set.get().set_key(CERTIFICATE_CHAIN);
        Value::List(vec![
            Value::Bytes(certificate.to_vec()),
            Value::Bytes(authorities.client_authority_der.to_vec()),
        ])
        .write_to_builder(set.get().init_value())?;
        set.get()
            .set_flags(dusk_program_kvs_internal::FLAG_PERSISTENT);
        set.get().set_forbidden_unstick(true);
        set.send().promise.await?;
        Ok(fingerprint(&certificate))
    })
    .await
}

struct LinkContext {
    connection: usize,
    mode: LinkMode,
    authorities: Arc<Authorities>,
    events: Sender<Event>,
}

async fn serve_link(mut stream: Async<TcpStream>, acceptor: TlsAcceptor, context: LinkContext) {
    let LinkContext {
        connection,
        mode,
        authorities,
        events,
    } = context;
    events.send(Event::Accepted { connection }).ok();
    if mode == LinkMode::NoHandshake {
        let mut buffer = [0u8; 4096];
        while let Ok(read) = futures::AsyncReadExt::read(&mut stream, &mut buffer).await {
            if read == 0 {
                break;
            }
        }
        return;
    }
    let mut stream = match acceptor.accept(stream).await {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("the fake fleet refused a handshake: {error}");
            events
                .send(Event::HandshakeFailed { listener: "fleet" })
                .ok();
            return;
        }
    };
    let started = Instant::now();
    let certificate = stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|chain| chain.first())
        .map(|certificate| fingerprint(certificate))
        .unwrap_or_default();
    let server_name = stream.get_ref().1.server_name().map(String::from);
    if mode == LinkMode::Silent {
        let mut buffer = [0u8; 4096];
        while let Ok(read) = stream.read(&mut buffer).await {
            if read == 0 {
                break;
            }
        }
        events
            .send(Event::LinkClosed {
                connection,
                lived: started.elapsed(),
            })
            .ok();
        return;
    }
    let (reader, writer) = stream.split();
    let network = twoparty::VatNetwork::new(reader, writer, Side::Client, Default::default());
    let mut rpc_system = RpcSystem::new(Box::new(network), None);
    let node: dusk::Client = rpc_system.bootstrap(Side::Server);
    let work = async {
        let namespace_id = node
            .namespace_id_request()
            .send()
            .promise
            .await?
            .get()?
            .get_result();
        let hostname = text(
            node.hostname_request()
                .send()
                .promise
                .await?
                .get()?
                .get_result(),
        )?;
        events
            .send(Event::Linked {
                connection,
                certificate,
                namespace_id,
                hostname,
                server_name,
            })
            .ok();
        match mode {
            LinkMode::DropAfterLinked => return Ok::<(), capnp::Error>(()),
            LinkMode::StoredThenServe | LinkMode::StoredThenStopNode => {
                let stored = with_kvs(&node, read_stored).await?;
                events
                    .send(Event::Stored {
                        connection,
                        stored: Box::new(stored),
                    })
                    .ok();
                if mode == LinkMode::StoredThenStopNode {
                    stop_node(&node).await?;
                    events.send(Event::Stopped).ok();
                }
            }
            LinkMode::ReplaceCertificateThenStopNode { validity_seconds } => {
                let certificate =
                    replace_certificate(&node, &authorities, validity_seconds).await?;
                events.send(Event::Replaced { certificate }).ok();
                stop_node(&node).await?;
                events.send(Event::Stopped).ok();
            }
            LinkMode::Serve | LinkMode::Silent | LinkMode::NoHandshake => {}
        }
        loop {
            Timer::after(Duration::from_millis(500)).await;
            node.time_request().send().promise.await?;
        }
    };
    futures::pin_mut!(work);
    if let futures::future::Either::Right((Err(error), _)) =
        futures::future::select(rpc_system, work).await
    {
        eprintln!("link {connection} ended with {error}");
    }
    events
        .send(Event::LinkClosed {
            connection,
            lived: started.elapsed(),
        })
        .ok();
}

pub(crate) struct Fleet {
    authorities: Arc<Authorities>,
    fleet_port: u16,
    provision_port: u16,
    pub(crate) directory: PathBuf,
    pub(crate) tpm: Option<PathBuf>,
    events: Receiver<Event>,
    pub(crate) seen: Vec<Event>,
}

impl Fleet {
    pub(crate) fn start(name: &str, behaviour: Behaviour) -> Fleet {
        let authorities = Arc::new(Authorities::new());
        let directory = std::env::temp_dir().join(format!(
            "dusk-nightfall-fleet-link-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("fleet-server-ca.pem"),
            &authorities.server_authority_pem,
        )
        .unwrap();
        let fleet_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let provision_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let fleet_port = fleet_listener.local_addr().unwrap().port();
        let provision_port = provision_listener.local_addr().unwrap().port();
        let (sender, events) = channel();
        let server_authorities = authorities.clone();
        std::thread::spawn(move || {
            let mut pool = futures::executor::LocalPool::new();
            let spawner = pool.spawner();
            let mut roots = rustls::RootCertStore::empty();
            roots
                .add(server_authorities.client_authority_der.clone())
                .unwrap();
            let fleet_verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
                Arc::new(roots),
                provider(),
            )
            .build()
            .unwrap();
            let fleet_versions: &[&'static rustls::SupportedProtocolVersion] =
                if behaviour.fleet_offers_only_tls12 {
                    &[&rustls::version::TLS12]
                } else {
                    &[&rustls::version::TLS13]
                };
            let fleet_acceptor =
                TlsAcceptor::from(server_authorities.server_config(fleet_verifier, fleet_versions));
            let provision_acceptor = TlsAcceptor::from(server_authorities.server_config(
                Arc::new(RecordAnyClientCertificate(provider())),
                &[&rustls::version::TLS13, &rustls::version::TLS12],
            ));
            let state = Rc::new(ProvisioningState {
                authorities: server_authorities.clone(),
                behaviour: behaviour.clone(),
                events: sender.clone(),
                challenges: RefCell::new(HashMap::new()),
            });
            let provision_spawner = spawner.clone();
            spawner
                .spawn_local(async move {
                    let listener = Async::new(provision_listener).unwrap();
                    loop {
                        let (stream, _) = listener.accept().await.unwrap();
                        provision_spawner
                            .spawn_local(serve_provisioning(
                                stream,
                                provision_acceptor.clone(),
                                state.clone(),
                            ))
                            .unwrap();
                    }
                })
                .unwrap();
            let fleet_spawner = spawner.clone();
            spawner
                .spawn_local(async move {
                    let listener = Async::new(fleet_listener).unwrap();
                    let mut connection = 0;
                    loop {
                        let (stream, _) = listener.accept().await.unwrap();
                        let mode = behaviour
                            .links
                            .get(connection)
                            .copied()
                            .unwrap_or(LinkMode::Serve);
                        connection += 1;
                        fleet_spawner
                            .spawn_local(serve_link(
                                stream,
                                fleet_acceptor.clone(),
                                LinkContext {
                                    connection,
                                    mode,
                                    authorities: server_authorities.clone(),
                                    events: sender.clone(),
                                },
                            ))
                            .unwrap();
                    }
                })
                .unwrap();
            pool.run();
        });
        Fleet {
            authorities,
            fleet_port,
            provision_port,
            directory,
            tpm: None,
            events,
            seen: Vec::new(),
        }
    }

    pub(crate) fn kvs_file(&self) -> PathBuf {
        self.directory.join("kvs")
    }

    pub(crate) fn command(&self, extra: &str) -> String {
        let tpm = match &self.tpm {
            Some(path) => format!(" --tpm {}", path.display()),
            None => String::from(" --no-tpm"),
        };
        format!(
            "nightfall -c 127.0.0.1:{} --server-name fleet.test --provision localhost:{} --provision-server-name provision.test --ca {}{tpm}{extra}",
            self.fleet_port,
            self.provision_port,
            self.directory.join("fleet-server-ca.pem").display(),
        )
    }

    pub(crate) fn start_node(&self, extra: &str) -> std::thread::JoinHandle<DuskImplExit> {
        start_node(self.command(extra), Some(self.kvs_file()))
    }

    pub(crate) fn expect(
        &mut self,
        timeout: Duration,
        what: &str,
        predicate: impl Fn(&Event) -> bool,
    ) -> Event {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(remaining) {
                Ok(event) => {
                    self.seen.push(event.clone());
                    if predicate(&event) {
                        return event;
                    }
                }
                Err(_) => panic!(
                    "no {what} within {timeout:?}; events so far: {:#?}",
                    self.seen
                ),
            }
        }
    }

    pub(crate) fn quiet(
        &mut self,
        period: Duration,
        what: &str,
        predicate: impl Fn(&Event) -> bool,
    ) {
        let deadline = Instant::now() + period;
        while let Ok(event) = self
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            self.seen.push(event.clone());
            assert!(
                !predicate(&event),
                "unexpected {what}: {event:?}; events so far: {:#?}",
                self.seen
            );
        }
    }

    pub(crate) fn stored(&mut self, connection: usize) -> Stored {
        let Event::Stored { stored, .. } = self.expect(
            Duration::from_secs(10),
            "the node's stored identity",
            |event| matches!(event, Event::Stored { connection: stored, .. } if *stored == connection),
        ) else {
            unreachable!()
        };
        *stored
    }

    pub(crate) fn provisioning_count(&self) -> usize {
        self.seen
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    Event::AssignAttempted
                        | Event::Assigned { .. }
                        | Event::Enrolled { .. }
                        | Event::Renewed { .. }
                        | Event::RenewRefused { .. }
                )
            })
            .count()
    }
}

impl Drop for Fleet {
    fn drop(&mut self) {
        if !std::thread::panicking()
            && let Err(error) = std::fs::remove_dir_all(&self.directory)
        {
            eprintln!("couldn't remove {}: {error}", self.directory.display());
        }
    }
}

pub(crate) fn start_node(
    script: String,
    kvs_file: Option<PathBuf>,
) -> std::thread::JoinHandle<DuskImplExit> {
    dusk_base::link_anchors();
    std::thread::spawn(move || {
        let disconnected: dusk::Client = capnp_rpc::new_future_client(async {
            Err(capnp::Error::disconnected(String::from(
                "the test compiles its init script without a node",
            )))
        });
        let init_script =
            futures::executor::block_on(dusk_program_sh::compile_to_words(disconnected, &script))
                .expect("compile the init script");
        let init = dusk_base::dusk_program_init::Args::new(&init_script)
            .expect("build the init args")
            .as_program_args()
            .expect("build the init program args");
        let persistent = kvs_file.map(|path| path.to_str().unwrap().to_string());
        dusk_nix::run(
            dusk_program::handle::new_handle(),
            move || {
                dusk_base::launcher_set(dusk_base::dusk_program_kvs::KvsConfig {
                    persistent: persistent.clone(),
                })
            },
            init,
        )
    })
}

pub(crate) fn is_machine_id(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty()
        && !trimmed.eq_ignore_ascii_case("uninitialized")
        && !trimmed
            .chars()
            .all(|character| character == '0' || character == '-')
}

pub(crate) fn join_stopped(node: std::thread::JoinHandle<DuskImplExit>) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !node.is_finished() {
        assert!(
            Instant::now() < deadline,
            "the stopped node is still running"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(node.join().unwrap(), DuskImplExit::Code(0));
}
