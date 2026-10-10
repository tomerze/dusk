use std::prelude::rust_2024::*;

use std::sync::Arc;
use std::time::Duration;

use dusk_capnp::capnp_rpc::{RpcSystem, rpc_twoparty_capnp, twoparty};
use dusk_program::embassy_time;
use dusk_program_kvs_internal::Kvs;
use futures::AsyncReadExt as _;
use rustls::pki_types::{CertificateDer, ServerName};

use crate::identity::{self, Identity};
use crate::node_key::NodeKey;
use crate::provision_capnp::{
    RENEW_BEYOND_GRACE, assignment, credential, device_report, provisioning,
};
use crate::tls::{self, HostPort};
use crate::tpm::Endorsement;

pub(crate) const REFUSAL_PREFIX: &str = "denied: ";
pub(crate) const CALL_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const MAXIMUM_CHAIN_LENGTH: u32 = 8;
const TRAVERSAL_LIMIT_IN_WORDS: usize = 128 * 1024;

#[derive(Debug, Clone, Default)]
pub(crate) struct DeviceReport {
    pub(crate) hardware_fingerprint: Vec<u8>,
    pub(crate) installation_hint: String,
    pub(crate) dusk_version: String,
    pub(crate) impl_name: String,
    pub(crate) target_os: String,
    pub(crate) target_arch: String,
    pub(crate) hostname: String,
    pub(crate) tpm: Option<TpmAttestation>,
}

#[derive(Debug, Clone)]
pub(crate) struct TpmAttestation {
    pub(crate) endorsement_key: Vec<u8>,
    pub(crate) endorsement_certificate: Vec<u8>,
    pub(crate) endorsement_certificate_chain: Vec<Vec<u8>>,
    pub(crate) node_key: Vec<u8>,
}

pub(crate) enum Token {
    Fleet(String),
    Install(String),
}

impl Token {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Token::Fleet(_) => "fleet token",
            Token::Install(_) => "install token",
        }
    }

    fn write(&self, mut builder: credential::Builder<'_>) {
        match self {
            Token::Fleet(token) => builder.set_fleet_token(token),
            Token::Install(token) => builder.set_install_token(token),
        }
    }
}

impl DeviceReport {
    fn write(&self, mut builder: device_report::Builder<'_>) {
        builder.set_hardware_fingerprint(&self.hardware_fingerprint);
        builder.set_installation_hint(&self.installation_hint);
        builder.set_dusk_version(&self.dusk_version);
        builder.set_impl(&self.impl_name);
        builder.set_target_os(&self.target_os);
        builder.set_target_arch(&self.target_arch);
        builder.set_hostname(&self.hostname);
        if let Some(evidence) = &self.tpm {
            let mut tpm = builder.init_tpm();
            tpm.set_endorsement_key(&evidence.endorsement_key);
            tpm.set_endorsement_certificate(&evidence.endorsement_certificate);
            tpm.set_node_key(&evidence.node_key);
            let mut chain = tpm.init_endorsement_certificate_chain(
                u32::try_from(evidence.endorsement_certificate_chain.len()).unwrap_or(u32::MAX),
            );
            for (index, certificate) in (0..).zip(&evidence.endorsement_certificate_chain) {
                chain.set(index, certificate);
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum ProvisioningError {
    Refused(String),
    Failed(anyhow::Error),
}

impl ProvisioningError {
    pub(crate) fn is_beyond_grace(&self) -> bool {
        matches!(self, ProvisioningError::Refused(reason) if reason.starts_with(RENEW_BEYOND_GRACE))
    }
}

impl core::fmt::Display for ProvisioningError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProvisioningError::Refused(reason) => write!(formatter, "refused: {reason}"),
            ProvisioningError::Failed(error) => write!(formatter, "{error:#}"),
        }
    }
}

impl From<anyhow::Error> for ProvisioningError {
    fn from(error: anyhow::Error) -> Self {
        ProvisioningError::Failed(error)
    }
}

pub(crate) fn classify(error: capnp::Error) -> ProvisioningError {
    let reason = error
        .extra
        .strip_prefix("remote exception: ")
        .unwrap_or(&error.extra);
    if error.kind == capnp::ErrorKind::Failed
        && (reason.starts_with(REFUSAL_PREFIX) || reason.starts_with(RENEW_BEYOND_GRACE))
    {
        ProvisioningError::Refused(reason.to_string())
    } else {
        ProvisioningError::Failed(anyhow::Error::new(error))
    }
}

pub(crate) struct Provisioner {
    pub(crate) target: HostPort,
    pub(crate) server_name: ServerName<'static>,
    pub(crate) trust_anchors: String,
}

impl Provisioner {
    async fn session<T>(
        &self,
        client_certificate: Option<Arc<rustls::sign::CertifiedKey>>,
        calls: impl AsyncFnOnce(provisioning::Client) -> Result<T, ProvisioningError>,
    ) -> Result<T, ProvisioningError> {
        let roots = tls::trust_anchors(&self.trust_anchors).await?;
        let config = tls::client_config(
            roots,
            &[&rustls::version::TLS13, &rustls::version::TLS12],
            client_certificate,
        )?;
        let connected = tls::connect_tls(&self.target, self.server_name.clone(), config).await?;
        let (reader, writer) = connected.stream.split();
        let mut options = capnp::message::ReaderOptions::new();
        options
            .traversal_limit_in_words(Some(TRAVERSAL_LIMIT_IN_WORDS))
            .nesting_limit(64);
        let network =
            twoparty::VatNetwork::new(reader, writer, rpc_twoparty_capnp::Side::Client, options);
        let mut rpc_system = RpcSystem::new(Box::new(network), None);
        let client: provisioning::Client = rpc_system.bootstrap(rpc_twoparty_capnp::Side::Server);
        let work = calls(client);
        futures::pin_mut!(work);
        match futures::future::select(rpc_system, work).await {
            futures::future::Either::Left((ended, _)) => {
                Err(ProvisioningError::Failed(match ended {
                    Ok(()) => anyhow::anyhow!("nightfall closed the provisioning connection"),
                    Err(error) => {
                        anyhow::Error::new(error).context("the provisioning connection failed")
                    }
                }))
            }
            futures::future::Either::Right((result, _)) => result,
        }
    }

    pub(crate) async fn enroll(
        &self,
        token: &Token,
        report: &DeviceReport,
        key: NodeKey,
        endorsement: Option<&Endorsement>,
        kvs: &Kvs,
    ) -> Result<Identity, ProvisioningError> {
        identity::stage(kvs, &key).await?;
        let key = Arc::new(key);
        let enrolled = self.session(None, async |client: provisioning::Client| {
            let mut assign = client.assign_request();
            {
                let mut parameters = assign.get();
                token.write(parameters.reborrow().init_credential());
                report.write(parameters.init_device());
            }
            let response = call(assign.send().promise).await?;
            let assignment = response.get().map_err(classify)?.get_assignment().map_err(classify)?;
            let device_id = text(assignment.get_device_id())?;
            let installation_id = text(assignment.get_installation_id())?;
            if !identity::is_identifier(&device_id) || !identity::is_identifier(&installation_id) {
                return Err(ProvisioningError::Failed(anyhow::anyhow!(
                    "nightfall assigned a device id `{device_id}` and installation id `{installation_id}` that are not 32 lowercase hex digits"
                )));
            }
            tracing::info!(device_id, installation_id, "nightfall assigned an identity");
            let challenge = challenge(assignment, &key, endorsement)?;
            let request = identity::certificate_request(&device_id, &installation_id, &key)?;
            let mut enroll = client.enroll_request();
            {
                let mut parameters = enroll.get();
                token.write(parameters.reborrow().init_credential());
                report.write(parameters.reborrow().init_device());
                parameters.set_challenge(&challenge);
                parameters.set_csr(&request);
            }
            let response = call(enroll.send().promise).await?;
            let chain = issued_chain(response.get().map_err(classify)?.get_issued().map_err(classify)?)?;
            let leaf = verified_leaf(&chain, &device_id, &installation_id, key.public_key())?;
            Ok(Identity {
                leaf,
                chain,
                key: key.clone(),
            })
        })
        .await?;
        identity::store(kvs, &enrolled).await?;
        Ok(enrolled)
    }

    pub(crate) async fn renew(
        &self,
        current: &Identity,
        kvs: &Kvs,
    ) -> Result<Identity, ProvisioningError> {
        let key = current.key.generate_like()?;
        identity::stage(kvs, &key).await?;
        let key = Arc::new(key);
        let renewed = self
            .session(
                Some(current.certified_key()),
                async |client: provisioning::Client| {
                    let request = identity::certificate_request(
                        current.device_id(),
                        current.installation_id(),
                        &key,
                    )?;
                    let mut renew = client.renew_request();
                    renew.get().set_csr(&request);
                    let response = call(renew.send().promise).await?;
                    let chain = issued_chain(
                        response
                            .get()
                            .map_err(classify)?
                            .get_issued()
                            .map_err(classify)?,
                    )?;
                    let leaf = verified_leaf(
                        &chain,
                        current.device_id(),
                        current.installation_id(),
                        key.public_key(),
                    )?;
                    Ok(Identity {
                        leaf,
                        chain,
                        key: key.clone(),
                    })
                },
            )
            .await?;
        identity::store(kvs, &renewed).await?;
        Ok(renewed)
    }
}

fn challenge(
    assignment: assignment::Reader<'_>,
    key: &NodeKey,
    endorsement: Option<&Endorsement>,
) -> Result<Vec<u8>, ProvisioningError> {
    let credential_blob = assignment.get_credential_blob().map_err(classify)?;
    if credential_blob.is_empty() {
        if endorsement.is_some() {
            tracing::info!(
                "nightfall asked for no TPM attestation; enrolling the TPM's key with the token alone"
            );
        }
        return Ok(assignment.get_challenge().map_err(classify)?.to_vec());
    }
    let (Some(endorsement), Some(object)) = (endorsement, key.tpm_object()) else {
        return Err(ProvisioningError::Failed(anyhow::anyhow!(
            "nightfall sent a TPM credential to a node that sent no TPM evidence"
        )));
    };
    let encrypted_secret = assignment.get_encrypted_secret().map_err(classify)?;
    let challenge = object
        .activate_credential(&endorsement.key, credential_blob, encrypted_secret)
        .map_err(|error| {
            ProvisioningError::Failed(
                error.context("the TPM did not release nightfall's challenge"),
            )
        })?;
    tracing::info!("the TPM released nightfall's challenge");
    Ok(challenge)
}

async fn call<T>(
    promise: impl core::future::Future<Output = Result<T, capnp::Error>>,
) -> Result<T, ProvisioningError> {
    embassy_time::with_timeout(tls::embassy_duration(CALL_TIMEOUT), promise)
        .await
        .map_err(|_| {
            ProvisioningError::Failed(anyhow::anyhow!(
                "nightfall did not answer within {} s",
                CALL_TIMEOUT.as_secs()
            ))
        })?
        .map_err(classify)
}

fn text(reader: capnp::Result<capnp::text::Reader<'_>>) -> Result<String, ProvisioningError> {
    let reader = reader.map_err(classify)?;
    reader.to_string().map_err(|error| {
        ProvisioningError::Failed(anyhow::anyhow!(
            "nightfall sent text that is not UTF-8: {error}"
        ))
    })
}

fn issued_chain(
    issued: crate::provision_capnp::issued::Reader<'_>,
) -> Result<Vec<CertificateDer<'static>>, ProvisioningError> {
    let list = issued.get_certificate_chain().map_err(classify)?;
    if list.is_empty() || list.len() > MAXIMUM_CHAIN_LENGTH {
        return Err(ProvisioningError::Failed(anyhow::anyhow!(
            "nightfall issued a chain of {} certificates; expected 1 to {MAXIMUM_CHAIN_LENGTH}",
            list.len()
        )));
    }
    list.iter()
        .map(|certificate| {
            certificate
                .map(|der| CertificateDer::from(der.to_vec()))
                .map_err(classify)
        })
        .collect()
}

fn verified_leaf(
    chain: &[CertificateDer<'static>],
    device_id: &str,
    installation_id: &str,
    public_key: &[u8],
) -> Result<identity::LeafCertificate, ProvisioningError> {
    let leaf = identity::parse_leaf(&chain[0]).map_err(|reason| {
        ProvisioningError::Failed(anyhow::anyhow!(
            "nightfall issued an unusable certificate: {reason}"
        ))
    })?;
    if leaf.identity.device_id != device_id || leaf.identity.installation_id != installation_id {
        return Err(ProvisioningError::Failed(anyhow::anyhow!(
            "nightfall issued a certificate for device {} installation {} instead of device {device_id} installation {installation_id}",
            leaf.identity.device_id,
            leaf.identity.installation_id
        )));
    }
    if leaf.public_key != public_key {
        return Err(ProvisioningError::Failed(anyhow::anyhow!(
            "nightfall issued a certificate for another key"
        )));
    }
    Ok(leaf)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE: &str = "00112233445566778899aabbccddeeff";
    const INSTALLATION: &str = "ffeeddccbbaa99887766554433221100";
    const ANOTHER: &str = "0123456789abcdef0123456789abcdef";

    fn new_key() -> rcgen::KeyPair {
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap()
    }

    fn leaf(
        device_id: &str,
        installation_id: &str,
        key: &rcgen::KeyPair,
    ) -> CertificateDer<'static> {
        let mut parameters = rcgen::CertificateParams::default();
        parameters.distinguished_name = rcgen::DistinguishedName::new();
        parameters.subject_alt_names = [
            format!("{}{device_id}", identity::DEVICE_URI_PREFIX),
            format!("{}{installation_id}", identity::INSTALLATION_URI_PREFIX),
        ]
        .into_iter()
        .map(|uri| rcgen::SanType::URI(rcgen::string::Ia5String::try_from(uri).unwrap()))
        .collect();
        parameters.self_signed(key).unwrap().der().clone()
    }

    fn issued(count: u32) -> capnp::message::TypedBuilder<crate::provision_capnp::issued::Owned> {
        let mut message =
            capnp::message::TypedBuilder::<crate::provision_capnp::issued::Owned>::new_default();
        let mut chain = message.init_root().init_certificate_chain(count);
        for index in 0..count {
            chain.set(index, &[0x30, 0x00]);
        }
        message
    }

    #[test]
    fn an_issued_chain_holds_1_to_8_certificates() {
        for count in [1, MAXIMUM_CHAIN_LENGTH] {
            let message = issued(count);
            let chain = issued_chain(message.get_root_as_reader().unwrap()).unwrap();
            assert_eq!(chain.len(), usize::try_from(count).unwrap());
        }
        for count in [0, MAXIMUM_CHAIN_LENGTH + 1] {
            let message = issued(count);
            assert!(
                issued_chain(message.get_root_as_reader().unwrap()).is_err(),
                "{count}"
            );
        }
    }

    #[test]
    fn the_issued_leaf_names_the_assigned_ids_and_the_new_key() {
        let key = new_key();
        let public_key = key.public_key_raw();
        let accepted = verified_leaf(
            &[leaf(DEVICE, INSTALLATION, &key)],
            DEVICE,
            INSTALLATION,
            public_key,
        )
        .unwrap();
        assert_eq!(accepted.identity.device_id, DEVICE);
        assert_eq!(accepted.identity.installation_id, INSTALLATION);
        for (chain, what) in [
            (leaf(ANOTHER, INSTALLATION, &key), "another device"),
            (leaf(DEVICE, ANOTHER, &key), "another installation"),
            (leaf(DEVICE, INSTALLATION, &new_key()), "another key"),
            (
                CertificateDer::from(b"not a certificate".to_vec()),
                "no certificate",
            ),
        ] {
            assert!(
                verified_leaf(&[chain], DEVICE, INSTALLATION, public_key).is_err(),
                "{what}"
            );
        }
    }

    #[test]
    fn only_a_node_with_tpm_evidence_answers_a_tpm_credential() {
        let key = NodeKey::generate().unwrap();
        let mut message = capnp::message::TypedBuilder::<assignment::Owned>::new_default();
        message.init_root().set_challenge(&[7; 32]);
        assert_eq!(
            challenge(message.get_root_as_reader().unwrap(), &key, None).unwrap(),
            [7; 32]
        );
        {
            let mut assignment = message.init_root();
            assignment.set_credential_blob(&[1; 68]);
            assignment.set_encrypted_secret(&[2; 256]);
        }
        assert!(matches!(
            challenge(message.get_root_as_reader().unwrap(), &key, None),
            Err(ProvisioningError::Failed(_))
        ));
    }

    #[test]
    fn denials_are_refusals_and_everything_else_is_a_failure() {
        let denied = classify(capnp::Error::failed(String::from(
            "remote exception: denied: device revoked",
        )));
        assert!(
            matches!(&denied, ProvisioningError::Refused(reason) if reason == "denied: device revoked")
        );
        assert!(!denied.is_beyond_grace());
        let beyond_grace = classify(capnp::Error::failed(format!(
            "remote exception: {RENEW_BEYOND_GRACE}"
        )));
        assert!(beyond_grace.is_beyond_grace());
        let mentions_grace = classify(capnp::Error::failed(String::from(
            "remote exception: denied: installation revoked during renew grace",
        )));
        assert!(!mentions_grace.is_beyond_grace());
        let undelimited = classify(capnp::Error::failed(String::from(
            "remote exception: deniedness is not a refusal",
        )));
        assert!(matches!(undelimited, ProvisioningError::Failed(_)));
        for error in [
            capnp::Error::overloaded(String::from("remote exception: denied: rate limited")),
            capnp::Error::disconnected(String::from("connection reset")),
            capnp::Error::failed(String::from("remote exception: step-ca is unreachable")),
            capnp::Error::unimplemented(String::from("remote exception: tpm attestation")),
        ] {
            assert!(matches!(classify(error), ProvisioningError::Failed(_)));
        }
    }
}
