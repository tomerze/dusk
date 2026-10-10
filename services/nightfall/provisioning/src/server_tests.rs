use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::provision_capnp::{RENEW_BEYOND_GRACE, provisioning};
use dusk_capnp::capnp;
use rustls_pki_types::CertificateDer;
use serde_json::{Value, json};
use time::OffsetDateTime;
use x509_parser::extensions::{GeneralName, ParsedExtension};

use crate::config::ProvisioningConfig;
use crate::credential::{CredentialKind, CredentialName, FleetTokens, InstallTokenKeys, sha256};
use crate::csr::fixtures::{csr, p256};
use crate::events::{EnrollmentEvent, EnrollmentEvents, Outcome};
use crate::fake_step_ca::FakeStepCa;
use crate::identity::{
    DeviceIdKey, TPM_ATTESTATION_URI, common_name, device_uri, installation_uri, tenant_uri,
};
use crate::jwt::SigningKey;
use crate::jwt::fixtures::{EdwardsSigner, private_jwk};
use crate::limits::PenaltyBox;
use crate::quota::{InstallationQuota, QuotaUnavailable, Reservation, ReservationOutcome};
use crate::server::{ConnectionInfo, Provisioning};
use crate::state::{Lifecycle, NodeStateView};
use crate::step_ca::{StepCaClient, StepCaConfig};
use crate::tpm::Evidence;
use crate::tpm::fixtures::{
    Authority, activate, endorsement_key_public, endorsement_private_key, modulus_of, node_key_of,
};

const KEY: &str = "0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b";
const INSTALLATION: &str = "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70";
const SUBJECT: &str = "kiosk-0042";
const ADDRESS: &str = "203.0.113.24:51730";

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<EnrollmentEvent>>,
    alerts: Mutex<Vec<(u64, bool)>>,
    collisions: Mutex<Vec<(String, usize, usize)>>,
}

impl EnrollmentEvents for Recorder {
    fn record(&self, event: EnrollmentEvent) {
        self.events.lock().unwrap().push(event);
    }

    fn rate_alert(&self, per_minute: u64, active: bool) {
        self.alerts.lock().unwrap().push((per_minute, active));
    }

    fn device_id_collision(&self, device_id: &str, installations: usize, addresses: usize) {
        self.collisions
            .lock()
            .unwrap()
            .push((String::from(device_id), installations, addresses));
    }
}

impl Recorder {
    fn last(&self) -> EnrollmentEvent {
        self.events.lock().unwrap().last().cloned().unwrap()
    }

    fn outcomes(&self) -> Vec<(Outcome, Option<String>)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|event| (event.outcome, event.reason.clone()))
            .collect()
    }
}

#[derive(Default)]
struct States {
    devices: Mutex<HashMap<String, Lifecycle>>,
    installations: Mutex<HashMap<(String, String), Lifecycle>>,
}

impl NodeStateView for States {
    fn device(&self, device_id: &str) -> Option<Lifecycle> {
        self.devices.lock().unwrap().get(device_id).copied()
    }

    fn installation(&self, device_id: &str, installation_id: &str) -> Option<Lifecycle> {
        self.installations
            .lock()
            .unwrap()
            .get(&(String::from(device_id), String::from(installation_id)))
            .copied()
    }
}

#[derive(Default)]
struct Penalties {
    penalized: Mutex<HashSet<IpAddr>>,
    failures: Mutex<Vec<IpAddr>>,
    enrollments: Mutex<Vec<IpAddr>>,
    enrollments_per_address: Mutex<Option<usize>>,
}

impl PenaltyBox for Penalties {
    fn penalized(&self, address: IpAddr) -> bool {
        self.penalized.lock().unwrap().contains(&address)
    }

    fn admit_enrollment(&self, address: IpAddr) -> bool {
        let mut enrollments = self.enrollments.lock().unwrap();
        enrollments.push(address);
        let used = enrollments.iter().filter(|seen| **seen == address).count();
        self.enrollments_per_address
            .lock()
            .unwrap()
            .is_none_or(|limit| used <= limit)
    }

    fn credential_failed(&self, address: IpAddr) {
        self.failures.lock().unwrap().push(address);
    }
}

#[derive(Default)]
struct Quota {
    used: Mutex<HashMap<CredentialName, u64>>,
    reservations: Mutex<Vec<Reservation>>,
    released: Mutex<Vec<Reservation>>,
    recorded: Mutex<Vec<Reservation>>,
    unavailable: Mutex<bool>,
    elsewhere: Mutex<u64>,
}

impl InstallationQuota for Quota {
    fn used(&self, credential: &CredentialName) -> Result<u64, QuotaUnavailable> {
        if *self.unavailable.lock().unwrap() {
            return Err(QuotaUnavailable(String::from("not caught up")));
        }
        Ok(self
            .used
            .lock()
            .unwrap()
            .get(credential)
            .copied()
            .unwrap_or(0))
    }

    fn reserve(&self, reservation: Reservation) -> ReservationOutcome {
        let mut used = self.used.lock().unwrap();
        let count = used.entry(reservation.credential.clone()).or_default();
        *count += std::mem::take(&mut *self.elsewhere.lock().unwrap());
        let granted = *count < reservation.limit;
        if granted {
            *count += 1;
        }
        self.reservations.lock().unwrap().push(reservation);
        Box::pin(async move { Ok(granted) })
    }

    fn release(&self, reservation: &Reservation) {
        if let Some(count) = self.used.lock().unwrap().get_mut(&reservation.credential) {
            *count = count.saturating_sub(1);
        }
        self.released.lock().unwrap().push(reservation.clone());
    }

    fn record(&self, reservation: &Reservation) {
        *self
            .used
            .lock()
            .unwrap()
            .entry(reservation.credential.clone())
            .or_default() += 1;
        self.recorded.lock().unwrap().push(reservation.clone());
    }
}

struct Harness {
    provisioning: Arc<Provisioning>,
    step_ca: FakeStepCa,
    events: Arc<Recorder>,
    states: Arc<States>,
    penalties: Arc<Penalties>,
    quota: Arc<Quota>,
    installer: EdwardsSigner,
    device_key: DeviceIdKey,
}

fn fleet_tokens() -> FleetTokens {
    FleetTokens::from_toml(&format!(
        "[[token]]\nname = \"retail-eu-2026\"\nvalue_sha256 = \"{}\"\ntenant = \"retail-eu\"\n\n[[token]]\nname = \"lab\"\nvalue_sha256 = \"{}\"\n",
        hex::encode(sha256(b"retail secret")),
        hex::encode(sha256(b"lab secret"))
    ))
    .unwrap()
}

async fn harness_with(adjust: impl FnOnce(&mut ProvisioningConfig, &mut StepCaConfig)) -> Harness {
    let key = SigningKey::from_jwk(&private_jwk()).unwrap();
    let step_ca = FakeStepCa::start(key.public_jwk()).await;
    let installer = EdwardsSigner::new("factory-2026");
    let mut config = ProvisioningConfig {
        instance: String::from("nightfall-0"),
        fleet_tokens: fleet_tokens(),
        install_token_keys: InstallTokenKeys::from_json(
            &json!({ "keys": [installer.public_jwk()] }).to_string(),
        )
        .unwrap(),
        device_id_key: DeviceIdKey::from_hex(KEY).unwrap(),
        fleet_client_roots: vec![step_ca.authority.root.clone()],
        endorsement_roots: Vec::new(),
        challenge_ttl: Duration::from_secs(300),
        challenge_capacity: 1000,
        renew_grace: Duration::from_secs(90 * 24 * 3600),
        certificate_lifetime: Duration::from_secs(168 * 3600),
        enrollments_per_second: 1000,
        enrollments_per_second_per_credential: 1000,
        enrollment_alert_per_minute: 1000,
    };
    let mut step_ca_config = StepCaConfig {
        url: step_ca.url.clone(),
        roots: vec![step_ca.tls_root.clone()],
        provisioner: String::from("nightfall"),
        provisioner_key: key,
        certificate_lifetime: Duration::from_secs(168 * 3600),
        max_concurrent: 4,
        timeout: Duration::from_secs(5),
    };
    adjust(&mut config, &mut step_ca_config);
    let events = Arc::new(Recorder::default());
    let states = Arc::new(States::default());
    let penalties = Arc::new(Penalties::default());
    let quota = Arc::new(Quota::default());
    let provisioning = Provisioning::new(
        config,
        StepCaClient::new(step_ca_config).unwrap(),
        events.clone(),
        states.clone(),
        penalties.clone(),
        quota.clone(),
    )
    .unwrap();
    Harness {
        provisioning,
        step_ca,
        events,
        states,
        penalties,
        quota,
        installer,
        device_key: DeviceIdKey::from_hex(KEY).unwrap(),
    }
}

async fn harness() -> Harness {
    harness_with(|_, _| {}).await
}

#[derive(Clone, Copy)]
enum Credential<'a> {
    Fleet(&'a str),
    Install(&'a str),
    Certificate,
}

struct Assigned {
    device_id: String,
    installation_id: String,
    challenge: Vec<u8>,
    expires_unix_ms: u64,
    credential_blob: Vec<u8>,
    encrypted_secret: Vec<u8>,
}

struct Issued {
    chain: Vec<Vec<u8>>,
    not_after_unix_ms: u64,
    renew_after_unix_ms: u64,
}

impl Harness {
    fn client(&self, address: &str, chain: Vec<CertificateDer<'static>>) -> provisioning::Client {
        self.provisioning.client(ConnectionInfo {
            remote_address: address.parse::<SocketAddr>().unwrap(),
            peer_certificates: chain,
        })
    }

    fn install_token(&self, subject: &str, token_id: &str, tenant: Option<&str>) -> String {
        let mut claims =
            json!({ "iss": "factory", "sub": subject, "jti": token_id, "exp": unix_now() + 3600 });
        if let Some(tenant) = tenant {
            claims["tenant"] = Value::from(tenant);
        }
        self.installer.sign(&claims)
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn set_credential(
    mut builder: crate::provision_capnp::credential::Builder<'_>,
    credential: Credential<'_>,
) {
    match credential {
        Credential::Fleet(token) => builder.set_fleet_token(token),
        Credential::Install(token) => builder.set_install_token(token),
        Credential::Certificate => builder.set_certificate(()),
    }
}

fn set_device(
    mut device: crate::provision_capnp::device_report::Builder<'_>,
    fingerprint: &[u8],
    tpm: Option<&Evidence>,
) {
    device.set_hardware_fingerprint(fingerprint);
    device.set_installation_hint("");
    device.set_dusk_version("0.1.0");
    device.set_impl("nix");
    device.set_target_os("linux");
    device.set_target_arch("x86_64");
    device.set_hostname("kiosk-0042");
    if let Some(evidence) = tpm {
        let mut attestation = device.init_tpm();
        attestation.set_endorsement_key(&evidence.endorsement_key);
        attestation.set_endorsement_certificate(&evidence.endorsement_certificate);
        let mut chain =
            attestation.reborrow().init_endorsement_certificate_chain(
                evidence.endorsement_certificate_chain.len() as u32,
            );
        for (index, certificate) in evidence.endorsement_certificate_chain.iter().enumerate() {
            chain.set(index as u32, certificate);
        }
        attestation.set_node_key(&evidence.node_key);
    }
}

async fn assign_device(
    client: &provisioning::Client,
    credential: Credential<'_>,
    fingerprint: &[u8],
    tpm: Option<&Evidence>,
) -> Result<Assigned, capnp::Error> {
    let mut request = client.assign_request();
    set_credential(request.get().init_credential(), credential);
    set_device(request.get().init_device(), fingerprint, tpm);
    let response = request.send().promise.await?;
    let assignment = response.get()?.get_assignment()?;
    Ok(Assigned {
        device_id: String::from(assignment.get_device_id()?.to_str()?),
        installation_id: String::from(assignment.get_installation_id()?.to_str()?),
        challenge: assignment.get_challenge()?.to_vec(),
        expires_unix_ms: assignment.get_challenge_expires_unix_ms(),
        credential_blob: assignment.get_credential_blob()?.to_vec(),
        encrypted_secret: assignment.get_encrypted_secret()?.to_vec(),
    })
}

async fn assign(
    client: &provisioning::Client,
    credential: Credential<'_>,
    fingerprint: &[u8],
) -> Result<Assigned, capnp::Error> {
    assign_device(client, credential, fingerprint, None).await
}

fn read_issued(issued: crate::provision_capnp::issued::Reader<'_>) -> Result<Issued, capnp::Error> {
    Ok(Issued {
        chain: issued
            .get_certificate_chain()?
            .iter()
            .map(|certificate| certificate.map(<[u8]>::to_vec))
            .collect::<Result<_, _>>()?,
        not_after_unix_ms: issued.get_not_after_unix_ms(),
        renew_after_unix_ms: issued.get_renew_after_unix_ms(),
    })
}

async fn enroll_device(
    client: &provisioning::Client,
    credential: Credential<'_>,
    fingerprint: &[u8],
    tpm: Option<&Evidence>,
    challenge: &[u8],
    csr_der: &[u8],
) -> Result<Issued, capnp::Error> {
    let mut request = client.enroll_request();
    set_credential(request.get().init_credential(), credential);
    set_device(request.get().init_device(), fingerprint, tpm);
    request.get().set_challenge(challenge);
    request.get().set_csr(csr_der);
    let response = request.send().promise.await?;
    read_issued(response.get()?.get_issued()?)
}

async fn enroll(
    client: &provisioning::Client,
    credential: Credential<'_>,
    fingerprint: &[u8],
    challenge: &[u8],
    csr_der: &[u8],
) -> Result<Issued, capnp::Error> {
    enroll_device(client, credential, fingerprint, None, challenge, csr_der).await
}

async fn renew(client: &provisioning::Client, csr_der: &[u8]) -> Result<Issued, capnp::Error> {
    let mut request = client.renew_request();
    request.get().set_csr(csr_der);
    let response = request.send().promise.await?;
    read_issued(response.get()?.get_issued()?)
}

fn leaf_uris(der: &[u8]) -> BTreeSet<String> {
    let (_, certificate) = x509_parser::parse_x509_certificate(der).unwrap();
    let mut uris = BTreeSet::new();
    for extension in certificate.extensions() {
        if let ParsedExtension::SubjectAlternativeName(names) = extension.parsed_extension() {
            for name in &names.general_names {
                if let GeneralName::URI(uri) = name {
                    uris.insert(String::from(*uri));
                }
            }
        }
    }
    uris
}

fn uris(device_id: &str, installation_id: &str) -> Vec<String> {
    vec![device_uri(device_id), installation_uri(installation_id)]
}

fn validator() -> jsonschema::Validator {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/kafka/dusk.enrollments.schema.json")
        .canonicalize()
        .unwrap();
    let schema: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    jsonschema::options()
        .with_base_uri(format!("file://{}", path.display()))
        .build(&schema)
        .unwrap()
}

fn assert_contract(events: &Recorder) {
    let validator = validator();
    for event in events.events.lock().unwrap().iter() {
        let message = event.message();
        let errors: Vec<String> = validator
            .iter_errors(&message)
            .map(|error| error.to_string())
            .collect();
        assert!(errors.is_empty(), "{message}: {errors:?}");
    }
}

fn denied_with(result: Result<impl Sized, capnp::Error>, text: &str) {
    match result {
        Ok(_) => panic!("expected a refusal containing {text}"),
        Err(error) => assert!(
            error.extra.contains(text),
            "{} does not contain {text}",
            error.extra
        ),
    }
}

#[tokio::test]
async fn enrolls_a_node_with_a_fleet_token() {
    let harness = harness().await;
    let client = harness.client(ADDRESS, Vec::new());
    let fingerprint = [1u8; 32];
    let assigned = assign(&client, Credential::Fleet("retail secret"), &fingerprint)
        .await
        .unwrap();
    assert_eq!(
        assigned.device_id,
        harness.device_key.device_id(&fingerprint)
    );
    assert_eq!(assigned.installation_id.len(), 32);
    assert_eq!(assigned.challenge.len(), 32);
    let now_ms = unix_now() * 1000;
    assert!(
        assigned.expires_unix_ms > now_ms + 290_000 && assigned.expires_unix_ms <= now_ms + 301_000
    );

    let key = p256();
    let request = csr(
        &key,
        None,
        &uris(&assigned.device_id, &assigned.installation_id),
    );
    let issued = enroll(
        &client,
        Credential::Fleet("retail secret"),
        &fingerprint,
        &assigned.challenge,
        &request,
    )
    .await
    .unwrap();
    assert_eq!(issued.chain.len(), 2);
    let mut expected = BTreeSet::from_iter(uris(&assigned.device_id, &assigned.installation_id));
    expected.insert(tenant_uri("retail-eu"));
    assert_eq!(leaf_uris(&issued.chain[0]), expected);
    let lifetime_ms = 168 * 3600 * 1000;
    assert!(issued.not_after_unix_ms.abs_diff(now_ms + lifetime_ms) < 10_000);
    let not_before_ms = issued.not_after_unix_ms - lifetime_ms - 60_000;
    let fraction =
        (issued.renew_after_unix_ms - not_before_ms) as f64 / (lifetime_ms + 60_000) as f64;
    assert!((0.55..=0.75).contains(&fraction), "{fraction}");

    let token = harness
        .step_ca
        .tokens
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert_eq!(
        token["sub"],
        common_name(&assigned.device_id, &assigned.installation_id).as_str()
    );
    assert_eq!(token["tenant"], "retail-eu");
    assert_eq!(token["jti"].as_str().unwrap().len(), 64);

    denied_with(
        enroll(
            &client,
            Credential::Fleet("retail secret"),
            &fingerprint,
            &assigned.challenge,
            &request,
        )
        .await,
        "invalid challenge",
    );
    let events = harness.events.events.lock().unwrap().clone();
    assert_eq!(events[0].outcome, Outcome::Assigned);
    assert_eq!(events[1].outcome, Outcome::Issued);
    assert_eq!(events[1].credential_ref.as_deref(), Some("retail-eu-2026"));
    assert_eq!(events[1].tenant.as_deref(), Some("retail-eu"));
    assert_eq!(events[1].hostname.as_deref(), Some("kiosk-0042"));
    assert_eq!(
        events[1].hardware_fingerprint_hash,
        Some(hex::encode(sha256(&fingerprint)))
    );
    assert_eq!(
        events[1].key(),
        Some(format!(
            "{}/{}",
            assigned.device_id, assigned.installation_id
        ))
    );
    assert_eq!(events[2].reason.as_deref(), Some("invalid_challenge"));
    assert_contract(&harness.events);
}

#[tokio::test]
async fn enrolls_with_a_single_use_install_token() {
    let harness = harness().await;
    let client = harness.client(ADDRESS, Vec::new());
    let token = harness.install_token(SUBJECT, "unit-1", None);
    let fingerprint = [2u8; 32];
    let assigned = assign(&client, Credential::Install(&token), &fingerprint)
        .await
        .unwrap();
    assert_eq!(assigned.installation_id.len(), 32);
    assert_eq!(
        harness.events.last().credential_ref.as_deref(),
        Some(SUBJECT)
    );
    let request = csr(
        &p256(),
        Some(&common_name(&assigned.device_id, &assigned.installation_id)),
        &uris(&assigned.device_id, &assigned.installation_id),
    );
    let issued = enroll(
        &client,
        Credential::Install(&token),
        &fingerprint,
        &assigned.challenge,
        &request,
    )
    .await
    .unwrap();
    assert_eq!(
        leaf_uris(&issued.chain[0]),
        BTreeSet::from_iter(uris(&assigned.device_id, &assigned.installation_id))
    );
    let claims = harness
        .step_ca
        .tokens
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    let material =
        b"\x00\x00\x00\x12dusk-install-token\x00\x00\x00\x07factory\x00\x00\x00\x06unit-1";
    assert_eq!(claims["jti"], hex::encode(sha256(material)).as_str());
    assert!(claims.get("tenant").is_none());

    let again = assign(&client, Credential::Install(&token), &fingerprint)
        .await
        .unwrap();
    assert_ne!(again.installation_id, assigned.installation_id);
    let request = csr(
        &p256(),
        None,
        &uris(&again.device_id, &again.installation_id),
    );
    denied_with(
        enroll(
            &client,
            Credential::Install(&token),
            &fingerprint,
            &again.challenge,
            &request,
        )
        .await,
        "certificate authority refused",
    );
    assert_eq!(harness.events.last().outcome, Outcome::Denied);
    assert_eq!(
        harness.events.last().reason.as_deref(),
        Some("step_ca_unauthorized")
    );
    assert_eq!(
        harness.events.last().credential_ref.as_deref(),
        Some(SUBJECT)
    );
    assert_contract(&harness.events);
}

#[tokio::test]
async fn refuses_bad_credentials_and_feeds_the_penalty_box() {
    let harness = harness().await;
    let client = harness.client(ADDRESS, Vec::new());
    let fingerprint = [3u8; 32];
    denied_with(
        assign(&client, Credential::Fleet("wrong"), &fingerprint).await,
        "invalid credential",
    );
    let stranger = EdwardsSigner::new("factory-2026");
    let forged = stranger
        .sign(&json!({ "iss": "factory", "sub": SUBJECT, "jti": "x", "exp": unix_now() + 60 }));
    denied_with(
        assign(&client, Credential::Install(&forged), &fingerprint).await,
        "invalid credential",
    );
    let address: SocketAddr = ADDRESS.parse().unwrap();
    assert_eq!(
        *harness.penalties.failures.lock().unwrap(),
        vec![address.ip(), address.ip()]
    );
    denied_with(
        assign(&client, Credential::Certificate, &fingerprint).await,
        "renew only",
    );
    denied_with(
        assign(&client, Credential::Fleet("retail secret"), &[3u8; 16]).await,
        "32 bytes",
    );
    harness
        .penalties
        .penalized
        .lock()
        .unwrap()
        .insert(address.ip());
    let limited = assign(&client, Credential::Fleet("retail secret"), &fingerprint)
        .await
        .err()
        .unwrap();
    assert_eq!(limited.kind, capnp::ErrorKind::Overloaded);
    assert_eq!(
        harness.events.outcomes(),
        vec![
            (Outcome::Denied, Some(String::from("invalid_credential"))),
            (Outcome::Denied, Some(String::from("invalid_install_token"))),
            (
                Outcome::Denied,
                Some(String::from("certificate_credential"))
            ),
            (
                Outcome::Denied,
                Some(String::from("invalid_hardware_fingerprint"))
            ),
            (Outcome::RateLimited, Some(String::from("penalty_box"))),
        ]
    );
    assert_eq!(harness.penalties.failures.lock().unwrap().len(), 2);
    assert_contract(&harness.events);
}

#[tokio::test]
async fn limits_enrollment_rates_and_raises_the_rate_alert() {
    let harness = harness_with(|config, _| {
        config.enrollments_per_second_per_credential = 1;
        config.enrollment_alert_per_minute = 2;
    })
    .await;
    let client = harness.client(ADDRESS, Vec::new());
    assign(&client, Credential::Fleet("retail secret"), &[4u8; 32])
        .await
        .unwrap();
    let limited = assign(&client, Credential::Fleet("retail secret"), &[4u8; 32])
        .await
        .err()
        .unwrap();
    assert_eq!(limited.kind, capnp::ErrorKind::Overloaded);
    assert_eq!(
        harness.events.last().reason.as_deref(),
        Some("credential_rate")
    );
    assign(&client, Credential::Fleet("lab secret"), &[4u8; 32])
        .await
        .unwrap();
    assert_eq!(*harness.events.alerts.lock().unwrap(), vec![(3, true)]);
    harness
        .provisioning
        .refresh_rate_alert(Instant::now() + Duration::from_secs(30));
    assert_eq!(harness.events.alerts.lock().unwrap().len(), 1);
    harness
        .provisioning
        .refresh_rate_alert(Instant::now() + Duration::from_secs(61));
    assert_eq!(
        *harness.events.alerts.lock().unwrap(),
        vec![(3, true), (0, false)]
    );
    assert_contract(&harness.events);
}

#[tokio::test]
async fn counts_one_enrollment_of_a_network_for_its_assign_and_enroll() {
    let harness = harness().await;
    *harness.penalties.enrollments_per_address.lock().unwrap() = Some(1);
    let client = harness.client(ADDRESS, Vec::new());
    let fingerprint = [13u8; 32];
    let device_id = harness.device_key.device_id(&fingerprint);
    let assigned = assign(&client, Credential::Fleet("lab secret"), &fingerprint)
        .await
        .unwrap();
    let request = csr(&p256(), None, &uris(&device_id, &assigned.installation_id));
    enroll(
        &client,
        Credential::Fleet("lab secret"),
        &fingerprint,
        &assigned.challenge,
        &request,
    )
    .await
    .unwrap();
    let limited = assign(&client, Credential::Fleet("lab secret"), &fingerprint)
        .await
        .err()
        .unwrap();
    assert_eq!(limited.kind, capnp::ErrorKind::Overloaded);
    assert_eq!(
        harness.events.last().reason.as_deref(),
        Some("network_enrollment_rate")
    );
    assert_eq!(harness.penalties.enrollments.lock().unwrap().len(), 2);
    assert_contract(&harness.events);
}

#[tokio::test]
async fn refuses_a_request_it_cannot_read_and_records_it() {
    let harness = harness().await;
    let client = harness.client(ADDRESS, Vec::new());
    let mut assign_request = client.assign_request();
    {
        let mut credential = assign_request.get().init_credential();
        let token = credential.reborrow().init_fleet_token(2);
        token.as_bytes_mut().copy_from_slice(&[0xff, 0xfe]);
    }
    set_device(assign_request.get().init_device(), &[14u8; 32], None);
    denied_with(assign_request.send().promise.await, "malformed request");
    let event = harness.events.last();
    assert_eq!(event.outcome, Outcome::Denied);
    assert_eq!(event.reason.as_deref(), Some("malformed_request"));
    assert_eq!(
        event.credential_kind,
        crate::credential::CredentialKind::FleetToken
    );
    let address: SocketAddr = ADDRESS.parse().unwrap();
    assert_eq!(
        *harness.penalties.failures.lock().unwrap(),
        vec![address.ip()]
    );
    assert_contract(&harness.events);
}

#[tokio::test]
async fn refuses_revoked_and_retired_nodes() {
    let harness = harness().await;
    let client = harness.client(ADDRESS, Vec::new());
    let fingerprint = [5u8; 32];
    let device_id = harness.device_key.device_id(&fingerprint);
    let assigned = assign(&client, Credential::Fleet("lab secret"), &fingerprint)
        .await
        .unwrap();
    harness
        .states
        .devices
        .lock()
        .unwrap()
        .insert(device_id.clone(), Lifecycle::Revoked);
    let request = csr(&p256(), None, &uris(&device_id, &assigned.installation_id));
    denied_with(
        enroll(
            &client,
            Credential::Fleet("lab secret"),
            &fingerprint,
            &assigned.challenge,
            &request,
        )
        .await,
        "device revoked",
    );
    denied_with(
        assign(&client, Credential::Fleet("lab secret"), &fingerprint).await,
        "device revoked",
    );
    harness
        .states
        .devices
        .lock()
        .unwrap()
        .insert(device_id.clone(), Lifecycle::Quarantined);
    let assigned = assign(&client, Credential::Fleet("lab secret"), &fingerprint)
        .await
        .unwrap();
    harness.states.installations.lock().unwrap().insert(
        (device_id.clone(), assigned.installation_id.clone()),
        Lifecycle::Retired,
    );
    let request = csr(&p256(), None, &uris(&device_id, &assigned.installation_id));
    denied_with(
        enroll(
            &client,
            Credential::Fleet("lab secret"),
            &fingerprint,
            &assigned.challenge,
            &request,
        )
        .await,
        "installation retired",
    );
    let reasons: Vec<Option<String>> = harness
        .events
        .outcomes()
        .into_iter()
        .map(|(_, reason)| reason)
        .collect();
    assert_eq!(reasons[1].as_deref(), Some("device_revoked"));
    assert_eq!(reasons[4].as_deref(), Some("installation_retired"));
    assert_contract(&harness.events);
}

#[tokio::test]
async fn binds_the_challenge_to_its_credential_fingerprint_and_ids() {
    let harness = harness().await;
    let client = harness.client(ADDRESS, Vec::new());
    let fingerprint = [6u8; 32];
    let device_id = harness.device_key.device_id(&fingerprint);
    let assigned = assign(&client, Credential::Fleet("lab secret"), &fingerprint)
        .await
        .unwrap();
    let request = csr(&p256(), None, &uris(&device_id, &assigned.installation_id));
    denied_with(
        enroll(
            &client,
            Credential::Fleet("lab secret"),
            &[7u8; 32],
            &assigned.challenge,
            &request,
        )
        .await,
        "invalid challenge",
    );
    let assigned = assign(&client, Credential::Fleet("lab secret"), &fingerprint)
        .await
        .unwrap();
    let request = csr(&p256(), None, &uris(&device_id, &assigned.installation_id));
    denied_with(
        enroll(
            &client,
            Credential::Fleet("retail secret"),
            &fingerprint,
            &assigned.challenge,
            &request,
        )
        .await,
        "invalid challenge",
    );
    let assigned = assign(&client, Credential::Fleet("lab secret"), &fingerprint)
        .await
        .unwrap();
    let other = csr(&p256(), None, &uris(&device_id, INSTALLATION));
    denied_with(
        enroll(
            &client,
            Credential::Fleet("lab secret"),
            &fingerprint,
            &assigned.challenge,
            &other,
        )
        .await,
        "must name exactly",
    );
    let request = csr(&p256(), None, &uris(&device_id, &assigned.installation_id));
    denied_with(
        enroll(
            &client,
            Credential::Fleet("lab secret"),
            &fingerprint,
            &assigned.challenge,
            &request,
        )
        .await,
        "invalid challenge",
    );
    let reasons: Vec<Option<String>> = harness
        .events
        .outcomes()
        .into_iter()
        .map(|(_, reason)| reason)
        .collect();
    assert_eq!(
        reasons,
        vec![
            None,
            Some(String::from("challenge_mismatch")),
            None,
            Some(String::from("challenge_mismatch")),
            None,
            Some(String::from("invalid_csr")),
            Some(String::from("invalid_challenge")),
        ]
    );
    assert_contract(&harness.events);
}

fn presented(
    harness: &Harness,
    key: &rcgen::KeyPair,
    device_id: &str,
    start_hours_ago: i64,
    lifetime_hours: i64,
) -> Vec<CertificateDer<'static>> {
    let start = OffsetDateTime::now_utc() - time::Duration::hours(start_hours_ago);
    let mut names = uris(device_id, INSTALLATION);
    names.push(tenant_uri("retail-eu"));
    let leaf = harness.step_ca.authority.leaf(
        key,
        &names,
        start,
        start + time::Duration::hours(lifetime_hours),
    );
    vec![leaf, harness.step_ca.authority.intermediate.clone()]
}

#[tokio::test]
async fn renews_with_a_new_key_and_keeps_the_tenant() {
    let harness = harness().await;
    let device_id = harness.device_key.device_id(&[8u8; 32]);
    let current = p256();
    let client = harness.client(ADDRESS, presented(&harness, &current, &device_id, 100, 168));
    let fresh = p256();
    let issued = renew(&client, &csr(&fresh, None, &uris(&device_id, INSTALLATION)))
        .await
        .unwrap();
    let mut expected = BTreeSet::from_iter(uris(&device_id, INSTALLATION));
    expected.insert(tenant_uri("retail-eu"));
    assert_eq!(leaf_uris(&issued.chain[0]), expected);
    let event = harness.events.last();
    assert_eq!(event.outcome, Outcome::Issued);
    assert_eq!(event.hardware_fingerprint_hash, None);
    assert_eq!(event.tenant.as_deref(), Some("retail-eu"));
    denied_with(
        renew(
            &client,
            &csr(&current, None, &uris(&device_id, INSTALLATION)),
        )
        .await,
        "new key",
    );
    denied_with(
        renew(
            &client,
            &csr(
                &fresh,
                None,
                &uris(&device_id, "00000000000000000000000000000000"),
            ),
        )
        .await,
        "must name exactly",
    );
    assert_contract(&harness.events);
}

#[tokio::test]
async fn applies_the_renew_window_grace_and_limit() {
    let harness = harness().await;
    let device_id = harness.device_key.device_id(&[9u8; 32]);
    let request = |key: &rcgen::KeyPair| csr(key, None, &uris(&device_id, INSTALLATION));
    let early = harness.client(ADDRESS, presented(&harness, &p256(), &device_id, 10, 168));
    let too_early = renew(&early, &request(&p256())).await.err().unwrap();
    assert_eq!(too_early.kind, capnp::ErrorKind::Overloaded);
    assert_eq!(
        harness.events.last().reason.as_deref(),
        Some("renew_too_early")
    );

    let expired = harness.client(ADDRESS, presented(&harness, &p256(), &device_id, 200, 168));
    renew(&expired, &request(&p256())).await.unwrap();
    let beyond = harness.client(
        ADDRESS,
        presented(&harness, &p256(), &device_id, 168 + 91 * 24, 168),
    );
    let refused = renew(&beyond, &request(&p256())).await.err().unwrap();
    assert_eq!(refused.extra, RENEW_BEYOND_GRACE);

    let unauthenticated = harness.client(ADDRESS, Vec::new());
    denied_with(
        renew(&unauthenticated, &request(&p256())).await,
        "TLS client certificate",
    );

    let current = harness.client(ADDRESS, presented(&harness, &p256(), &device_id, 100, 168));
    for _ in 0..3 {
        renew(&current, &request(&p256())).await.unwrap();
    }
    let limited = renew(&current, &request(&p256())).await.err().unwrap();
    assert_eq!(limited.kind, capnp::ErrorKind::Overloaded);
    assert_eq!(harness.events.last().reason.as_deref(), Some("renew_limit"));

    harness.states.installations.lock().unwrap().insert(
        (device_id.clone(), String::from(INSTALLATION)),
        Lifecycle::Revoked,
    );
    denied_with(
        renew(&current, &request(&p256())).await,
        "installation revoked",
    );
    assert_contract(&harness.events);
}

#[tokio::test]
async fn reports_step_ca_failures_timeouts_and_saturation() {
    let harness = harness_with(|_, step_ca| {
        step_ca.max_concurrent = 1;
        step_ca.timeout = Duration::from_millis(1500);
    })
    .await;
    let client = harness.client(ADDRESS, Vec::new());
    let fingerprint = [10u8; 32];
    let device_id = harness.device_key.device_id(&fingerprint);
    let attempt = || async {
        let assigned = assign(&client, Credential::Fleet("lab secret"), &fingerprint)
            .await
            .unwrap();
        let request = csr(&p256(), None, &uris(&device_id, &assigned.installation_id));
        enroll(
            &client,
            Credential::Fleet("lab secret"),
            &fingerprint,
            &assigned.challenge,
            &request,
        )
        .await
    };
    harness.step_ca.behaviour.lock().unwrap().status = Some(500);
    denied_with(attempt().await, "unavailable");
    assert_eq!(
        harness.events.last().reason.as_deref(),
        Some("step_ca_error")
    );
    harness.step_ca.behaviour.lock().unwrap().status = Some(401);
    denied_with(attempt().await, "certificate authority refused");
    assert_eq!(
        (
            harness.events.last().outcome,
            harness.events.last().reason.as_deref()
        ),
        (Outcome::Error, Some("step_ca_unauthorized"))
    );
    harness.step_ca.behaviour.lock().unwrap().status = None;
    harness.step_ca.behaviour.lock().unwrap().oversized = true;
    denied_with(attempt().await, "unavailable");
    assert_eq!(
        harness.events.last().reason.as_deref(),
        Some("step_ca_error")
    );
    harness.step_ca.behaviour.lock().unwrap().oversized = false;
    harness.step_ca.behaviour.lock().unwrap().delay = Some(Duration::from_secs(3));
    denied_with(attempt().await, "unavailable");
    assert_eq!(
        harness.events.last().reason.as_deref(),
        Some("step_ca_error")
    );
    harness.step_ca.behaviour.lock().unwrap().delay = Some(Duration::from_millis(300));
    let (first, second) = tokio::join!(attempt(), attempt());
    let kinds: BTreeSet<String> = [first, second]
        .into_iter()
        .map(|outcome| match outcome {
            Ok(_) => String::from("issued"),
            Err(error) => format!("{:?}", error.kind),
        })
        .collect();
    assert_eq!(
        kinds,
        BTreeSet::from([String::from("issued"), String::from("Overloaded")])
    );
    assert!(
        harness
            .events
            .outcomes()
            .contains(&(Outcome::Error, Some(String::from("step_ca_saturated"))))
    );
    assert_contract(&harness.events);
}

#[tokio::test]
async fn reports_a_device_id_enrolling_from_many_addresses() {
    let harness = harness().await;
    let fingerprint = [11u8; 32];
    let device_id = harness.device_key.device_id(&fingerprint);
    for number in 0..22u8 {
        let client = harness.client(&format!("198.51.100.{}:4000", number % 7), Vec::new());
        let assigned = assign(&client, Credential::Fleet("lab secret"), &fingerprint)
            .await
            .unwrap();
        let request = csr(&p256(), None, &uris(&device_id, &assigned.installation_id));
        enroll(
            &client,
            Credential::Fleet("lab secret"),
            &fingerprint,
            &assigned.challenge,
            &request,
        )
        .await
        .unwrap();
    }
    assert_eq!(
        *harness.events.collisions.lock().unwrap(),
        vec![(device_id, 21, 7)]
    );
}

fn tpm_device(root: &Authority) -> (Evidence, rcgen::KeyPair) {
    let modulus = modulus_of(endorsement_private_key());
    let node_key = p256();
    let now = OffsetDateTime::now_utc();
    let evidence = Evidence {
        endorsement_key: endorsement_key_public(&modulus),
        endorsement_certificate: root.endorsement_certificate(
            &modulus,
            now - time::Duration::days(1),
            now + time::Duration::days(3650),
            true,
        ),
        endorsement_certificate_chain: Vec::new(),
        node_key: node_key_of(&node_key),
    };
    (evidence, node_key)
}

fn node_key_name(evidence: &Evidence) -> Vec<u8> {
    let mut name = vec![0x00, 0x0b];
    name.extend_from_slice(&sha256(&evidence.node_key));
    name
}

async fn tpm_harness(root: &Authority) -> Harness {
    let certificate = root.certificate.clone();
    harness_with(move |config, _| {
        config.endorsement_roots = vec![CertificateDer::from(certificate)];
    })
    .await
}

#[tokio::test]
async fn enrolls_a_tpm_attested_node_and_marks_its_certificate() {
    let root = Authority::root("tpm manufacturer");
    let harness = tpm_harness(&root).await;
    let client = harness.client(ADDRESS, Vec::new());
    let (evidence, node_key) = tpm_device(&root);
    let fingerprint = sha256(&evidence.endorsement_key);
    let assigned = assign_device(
        &client,
        Credential::Fleet("retail secret"),
        &fingerprint,
        Some(&evidence),
    )
    .await
    .unwrap();
    assert_eq!(
        assigned.device_id,
        harness.device_key.device_id(&fingerprint)
    );
    assert!(assigned.challenge.is_empty());
    let secret = activate(
        endorsement_private_key(),
        &node_key_name(&evidence),
        &assigned.credential_blob,
        &assigned.encrypted_secret,
    )
    .unwrap();
    assert_eq!(secret.len(), 32);
    let request = csr(
        &node_key,
        None,
        &uris(&assigned.device_id, &assigned.installation_id),
    );
    let issued = enroll_device(
        &client,
        Credential::Fleet("retail secret"),
        &fingerprint,
        Some(&evidence),
        &secret,
        &request,
    )
    .await
    .unwrap();
    let mut expected = BTreeSet::from_iter(uris(&assigned.device_id, &assigned.installation_id));
    expected.insert(tenant_uri("retail-eu"));
    expected.insert(String::from(TPM_ATTESTATION_URI));
    assert_eq!(leaf_uris(&issued.chain[0]), expected);
    let claims = harness
        .step_ca
        .tokens
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert_eq!(claims["attestation"], "tpm");
    denied_with(
        enroll_device(
            &client,
            Credential::Fleet("retail secret"),
            &fingerprint,
            Some(&evidence),
            &secret,
            &request,
        )
        .await,
        "invalid challenge",
    );
    let events = harness.events.events.lock().unwrap().clone();
    assert_eq!(
        events
            .iter()
            .map(|event| (event.outcome, event.reason.clone()))
            .collect::<Vec<_>>(),
        vec![
            (Outcome::Assigned, None),
            (Outcome::Issued, None),
            (Outcome::Denied, Some(String::from("invalid_challenge"))),
        ]
    );
    assert!(
        events
            .iter()
            .all(|event| event.credential_kind == crate::credential::CredentialKind::FleetToken)
    );
    assert_eq!(
        events[1].hardware_fingerprint_hash,
        Some(hex::encode(sha256(&fingerprint)))
    );
    assert_contract(&harness.events);
}

#[tokio::test]
async fn refuses_a_csr_for_another_key_and_then_the_released_secret() {
    let root = Authority::root("tpm manufacturer");
    let harness = tpm_harness(&root).await;
    let client = harness.client(ADDRESS, Vec::new());
    let (evidence, node_key) = tpm_device(&root);
    let fingerprint = sha256(&evidence.endorsement_key);
    let assigned = assign_device(
        &client,
        Credential::Fleet("lab secret"),
        &fingerprint,
        Some(&evidence),
    )
    .await
    .unwrap();
    let secret = activate(
        endorsement_private_key(),
        &node_key_name(&evidence),
        &assigned.credential_blob,
        &assigned.encrypted_secret,
    )
    .unwrap();
    let names = uris(&assigned.device_id, &assigned.installation_id);
    denied_with(
        enroll_device(
            &client,
            Credential::Fleet("lab secret"),
            &fingerprint,
            Some(&evidence),
            &secret,
            &csr(&p256(), None, &names),
        )
        .await,
        "not the attested TPM key",
    );
    denied_with(
        enroll_device(
            &client,
            Credential::Fleet("lab secret"),
            &fingerprint,
            Some(&evidence),
            &secret,
            &csr(&node_key, None, &names),
        )
        .await,
        "invalid challenge",
    );
    assert_eq!(
        harness.events.outcomes(),
        vec![
            (Outcome::Assigned, None),
            (Outcome::Denied, Some(String::from("tpm_key_mismatch"))),
            (Outcome::Denied, Some(String::from("invalid_challenge"))),
        ]
    );
    assert!(harness.step_ca.tokens.lock().unwrap().is_empty());
    assert_contract(&harness.events);
}

#[tokio::test]
async fn refuses_tpm_evidence_it_cannot_verify() {
    let root = Authority::root("tpm manufacturer");
    let (evidence, _) = tpm_device(&root);
    let fingerprint = sha256(&evidence.endorsement_key);

    let unconfigured = harness().await;
    let client = unconfigured.client(ADDRESS, Vec::new());
    denied_with(
        assign_device(
            &client,
            Credential::Fleet("lab secret"),
            &fingerprint,
            Some(&evidence),
        )
        .await,
        "trusts no TPM endorsement key roots",
    );
    assert_eq!(
        unconfigured.events.last().reason.as_deref(),
        Some("tpm_roots_missing")
    );

    let harness = tpm_harness(&root).await;
    let client = harness.client(ADDRESS, Vec::new());
    let stranger = Authority::root("another manufacturer");
    let (foreign, _) = tpm_device(&stranger);
    let mut node_key = evidence.node_key.clone();
    node_key[5] |= 0x01;
    let cases = [
        (
            Evidence {
                endorsement_certificate: Vec::new(),
                ..evidence.clone()
            },
            fingerprint,
            "tpm_certificate_missing",
        ),
        (evidence.clone(), [7u8; 32], "tpm_fingerprint_mismatch"),
        (foreign, fingerprint, "tpm_certificate_untrusted"),
        (
            Evidence {
                node_key,
                ..evidence.clone()
            },
            fingerprint,
            "tpm_key_template",
        ),
    ];
    for (evidence, fingerprint, reason) in &cases {
        denied_with(
            assign_device(
                &client,
                Credential::Fleet("lab secret"),
                fingerprint,
                Some(evidence),
            )
            .await,
            "denied",
        );
        assert_eq!(harness.events.last().reason.as_deref(), Some(*reason));
    }
    assert!(harness.penalties.failures.lock().unwrap().is_empty());
    assert_contract(&harness.events);
}

#[tokio::test]
async fn limits_the_enrollments_of_one_endorsement_key() {
    let root = Authority::root("tpm manufacturer");
    let certificate = root.certificate.clone();
    let harness = harness_with(move |config, _| {
        config.endorsement_roots = vec![CertificateDer::from(certificate)];
        config.enrollments_per_second_per_credential = 2;
    })
    .await;
    let client = harness.client(ADDRESS, Vec::new());
    let (evidence, _) = tpm_device(&root);
    let fingerprint = sha256(&evidence.endorsement_key);
    for token in ["lab secret", "retail secret"] {
        assign_device(
            &client,
            Credential::Fleet(token),
            &fingerprint,
            Some(&evidence),
        )
        .await
        .unwrap();
    }
    let limited = assign_device(
        &client,
        Credential::Fleet("lab secret"),
        &fingerprint,
        Some(&evidence),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(limited.kind, capnp::ErrorKind::Overloaded);
    assert_eq!(
        harness.events.last().reason.as_deref(),
        Some("endorsement_key_rate")
    );
    assign(&client, Credential::Fleet("lab secret"), &[4u8; 32])
        .await
        .unwrap();
    assert_contract(&harness.events);
}

#[tokio::test]
async fn renews_a_tpm_bound_certificate_only_with_the_key_it_has() {
    let harness = harness().await;
    let device_id = harness.device_key.device_id(&[8u8; 32]);
    let current = p256();
    let start = OffsetDateTime::now_utc() - time::Duration::hours(100);
    let mut names = uris(&device_id, INSTALLATION);
    names.push(tenant_uri("retail-eu"));
    names.push(String::from(TPM_ATTESTATION_URI));
    let leaf =
        harness
            .step_ca
            .authority
            .leaf(&current, &names, start, start + time::Duration::hours(168));
    let client = harness.client(
        ADDRESS,
        vec![leaf, harness.step_ca.authority.intermediate.clone()],
    );
    let request_names = uris(&device_id, INSTALLATION);
    denied_with(
        renew(&client, &csr(&p256(), None, &request_names)).await,
        "needs the key it already has",
    );
    let issued = renew(&client, &csr(&current, None, &request_names))
        .await
        .unwrap();
    assert_eq!(
        leaf_uris(&issued.chain[0]),
        BTreeSet::from_iter(names.iter().cloned())
    );
    let claims = harness
        .step_ca
        .tokens
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert_eq!(claims["attestation"], "tpm");
    assert_eq!(
        harness.events.outcomes(),
        vec![
            (Outcome::Denied, Some(String::from("renew_key_changed"))),
            (Outcome::Issued, None),
        ]
    );
    assert_contract(&harness.events);
}

fn tokens_file(entries: &[(&str, &str, &str)]) -> FleetTokens {
    let mut text = String::new();
    for (name, secret, extra) in entries {
        text.push_str(&format!(
            "[[token]]\nname = \"{name}\"\nvalue_sha256 = \"{}\"\n{extra}\n",
            hex::encode(sha256(secret.as_bytes()))
        ));
    }
    FleetTokens::from_toml(&text).unwrap()
}

async fn enroll_new(
    client: &provisioning::Client,
    credential: Credential<'_>,
    fingerprint: [u8; 32],
) -> Result<Issued, capnp::Error> {
    let assigned = assign(client, credential, &fingerprint).await?;
    let request = csr(
        &p256(),
        None,
        &uris(&assigned.device_id, &assigned.installation_id),
    );
    enroll(
        client,
        credential,
        &fingerprint,
        &assigned.challenge,
        &request,
    )
    .await
}

fn reasons(events: &Recorder) -> Vec<Option<String>> {
    events
        .events
        .lock()
        .unwrap()
        .iter()
        .map(|event| event.reason.clone())
        .collect()
}

#[tokio::test]
async fn refuses_a_retired_fleet_token_at_once_and_without_a_penalty() {
    let harness = harness().await;
    let client = harness.client(ADDRESS, Vec::new());
    let fingerprint = [21u8; 32];
    let assigned = assign(&client, Credential::Fleet("retail secret"), &fingerprint)
        .await
        .unwrap();
    harness.provisioning.replace_fleet_tokens(tokens_file(&[
        (
            "retail-eu-2026",
            "retail secret",
            "tenant = \"retail-eu\"\nretired = true",
        ),
        ("lab", "lab secret", ""),
    ]));
    let request = csr(
        &p256(),
        None,
        &uris(&assigned.device_id, &assigned.installation_id),
    );
    denied_with(
        enroll(
            &client,
            Credential::Fleet("retail secret"),
            &fingerprint,
            &assigned.challenge,
            &request,
        )
        .await,
        "denied: credential retired",
    );
    denied_with(
        assign(&client, Credential::Fleet("retail secret"), &fingerprint).await,
        "denied: credential retired",
    );
    let refused = harness.events.last();
    assert_eq!(refused.outcome, Outcome::Denied);
    assert_eq!(refused.reason.as_deref(), Some("credential_retired"));
    assert_eq!(refused.credential_ref.as_deref(), Some("retail-eu-2026"));
    assert_eq!(refused.tenant.as_deref(), Some("retail-eu"));
    assert!(harness.penalties.failures.lock().unwrap().is_empty());
    enroll_new(&client, Credential::Fleet("lab secret"), [22u8; 32])
        .await
        .unwrap();
    assert_contract(&harness.events);
}

#[tokio::test]
async fn takes_added_removed_and_re_tenanted_fleet_tokens_without_a_restart() {
    let harness = harness().await;
    let client = harness.client(ADDRESS, Vec::new());
    harness.provisioning.replace_fleet_tokens(tokens_file(&[
        ("retail-eu-2026", "retail secret", "tenant = \"retail-us\""),
        ("batch-7", "batch secret", "tenant = \"acme\""),
    ]));
    let issued = enroll_new(&client, Credential::Fleet("batch secret"), [23u8; 32])
        .await
        .unwrap();
    assert!(leaf_uris(&issued.chain[0]).contains(&tenant_uri("acme")));
    assert_eq!(
        harness.events.last().credential_ref.as_deref(),
        Some("batch-7")
    );
    let issued = enroll_new(&client, Credential::Fleet("retail secret"), [24u8; 32])
        .await
        .unwrap();
    assert!(leaf_uris(&issued.chain[0]).contains(&tenant_uri("retail-us")));
    denied_with(
        assign(&client, Credential::Fleet("lab secret"), &[25u8; 32]).await,
        "invalid credential",
    );
    assert_eq!(
        harness.events.last().reason.as_deref(),
        Some("invalid_credential")
    );
    assert_contract(&harness.events);
}

#[tokio::test]
async fn counts_what_an_uncapped_token_enrolls_toward_a_cap_set_later() {
    let harness = harness().await;
    let client = harness.client(ADDRESS, Vec::new());
    harness.step_ca.behaviour.lock().unwrap().status = Some(500);
    denied_with(
        enroll_new(&client, Credential::Fleet("lab secret"), [26u8; 32]).await,
        "unavailable",
    );
    harness.step_ca.behaviour.lock().unwrap().status = None;
    enroll_new(&client, Credential::Fleet("lab secret"), [27u8; 32])
        .await
        .unwrap();
    let recorded = harness.quota.recorded.lock().unwrap().clone();
    assert_eq!(
        recorded.len(),
        1,
        "only the issued installation is recorded"
    );
    assert_eq!(recorded[0].limit, crate::quota::UNCAPPED);
    assert!(harness.quota.reservations.lock().unwrap().is_empty());
    harness.provisioning.replace_fleet_tokens(tokens_file(&[(
        "lab",
        "lab secret",
        "max_installations = 1",
    )]));
    denied_with(
        assign(&client, Credential::Fleet("lab secret"), &[28u8; 32]).await,
        "denied: credential quota reached",
    );
    assert_contract(&harness.events);
}

#[tokio::test]
async fn caps_the_installations_a_fleet_token_enrolls() {
    let harness = harness().await;
    harness.provisioning.replace_fleet_tokens(tokens_file(&[
        ("retail-eu-2026", "retail secret", "tenant = \"retail-eu\""),
        ("lab", "lab secret", "max_installations = 3"),
    ]));
    let client = harness.client(ADDRESS, Vec::new());
    for number in 0..2u8 {
        enroll_new(&client, Credential::Fleet("lab secret"), [30 + number; 32])
            .await
            .unwrap();
    }
    let fingerprint = [33u8; 32];
    let late = assign(&client, Credential::Fleet("lab secret"), &fingerprint)
        .await
        .unwrap();
    *harness.quota.elsewhere.lock().unwrap() = 1;
    let request = csr(&p256(), None, &uris(&late.device_id, &late.installation_id));
    denied_with(
        enroll(
            &client,
            Credential::Fleet("lab secret"),
            &fingerprint,
            &late.challenge,
            &request,
        )
        .await,
        "denied: credential quota reached",
    );
    denied_with(
        assign(&client, Credential::Fleet("lab secret"), &[34u8; 32]).await,
        "denied: credential quota reached",
    );
    enroll_new(&client, Credential::Fleet("retail secret"), [35u8; 32])
        .await
        .unwrap();
    let lab = CredentialName {
        kind: CredentialKind::FleetToken,
        name: String::from("lab"),
    };
    let reservations = harness.quota.reservations.lock().unwrap().clone();
    assert_eq!(reservations.len(), 3);
    assert!(
        reservations
            .iter()
            .all(|reservation| reservation.credential == lab && reservation.limit == 3)
    );
    assert_eq!(reservations[2].installation_id, late.installation_id);
    assert_eq!(harness.quota.used(&lab), Ok(3));
    let quota_refusals: Vec<EnrollmentEvent> = harness
        .events
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event.reason.as_deref() == Some("credential_quota_reached"))
        .cloned()
        .collect();
    assert_eq!(quota_refusals.len(), 2);
    assert!(quota_refusals.iter().all(|event| {
        event.outcome == Outcome::Denied && event.credential_ref.as_deref() == Some("lab")
    }));
    assert_eq!(
        harness.provisioning.quota_limits(),
        vec![(lab, 3)],
        "only the capped entry has a limit"
    );
    assert_contract(&harness.events);
}

#[tokio::test]
async fn gives_back_the_reserved_installation_when_signing_fails() {
    let harness = harness().await;
    harness.provisioning.replace_fleet_tokens(tokens_file(&[(
        "lab",
        "lab secret",
        "max_installations = 1",
    )]));
    let client = harness.client(ADDRESS, Vec::new());
    harness.step_ca.behaviour.lock().unwrap().status = Some(500);
    denied_with(
        enroll_new(&client, Credential::Fleet("lab secret"), [36u8; 32]).await,
        "unavailable",
    );
    assert_eq!(harness.quota.released.lock().unwrap().len(), 1);
    harness.step_ca.behaviour.lock().unwrap().status = None;
    enroll_new(&client, Credential::Fleet("lab secret"), [37u8; 32])
        .await
        .unwrap();
    assert_eq!(
        reasons(&harness.events),
        vec![None, Some(String::from("step_ca_error")), None, None]
    );
    assert_contract(&harness.events);
}

#[tokio::test]
async fn rate_limits_every_install_token_of_one_key_together() {
    let harness = harness_with(|config, _| {
        config.enrollments_per_second_per_credential = 2;
    })
    .await;
    let client = harness.client(ADDRESS, Vec::new());
    let first = harness.install_token("unit-1", "batch-9-unit-1", None);
    enroll_new(&client, Credential::Install(&first), [38u8; 32])
        .await
        .unwrap();
    let issued = harness.events.last();
    assert_eq!(issued.credential_ref.as_deref(), Some("unit-1"));
    assert_eq!(issued.credential_issuer.as_deref(), Some("factory-2026"));
    let second = harness.install_token("unit-2", "batch-9-unit-2", None);
    let limited = assign(&client, Credential::Install(&second), &[39u8; 32])
        .await
        .err()
        .unwrap();
    assert_eq!(limited.kind, capnp::ErrorKind::Overloaded);
    assert_eq!(
        harness.events.last().reason.as_deref(),
        Some("credential_rate")
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    assign(&client, Credential::Install(&second), &[39u8; 32])
        .await
        .unwrap();
    assert_contract(&harness.events);
}

#[tokio::test]
async fn caps_the_installations_of_an_install_token_key() {
    let harness = harness().await;
    let mut key = harness.installer.public_jwk();
    key["max_installations"] = Value::from(1);
    harness.provisioning.replace_install_token_keys(
        InstallTokenKeys::from_json(&json!({ "keys": [key] }).to_string()).unwrap(),
    );
    let client = harness.client(ADDRESS, Vec::new());
    let first = harness.install_token("unit-1", "batch-9-unit-1", None);
    enroll_new(&client, Credential::Install(&first), [38u8; 32])
        .await
        .unwrap();
    let second = harness.install_token("unit-2", "batch-9-unit-2", None);
    denied_with(
        assign(&client, Credential::Install(&second), &[39u8; 32]).await,
        "denied: credential quota reached",
    );
    let refused = harness.events.last();
    assert_eq!(refused.credential_ref.as_deref(), Some("unit-2"));
    assert_eq!(refused.credential_issuer.as_deref(), Some("factory-2026"));
    assert_eq!(
        harness.provisioning.quota_limits(),
        vec![(
            CredentialName {
                kind: CredentialKind::InstallToken,
                name: String::from("factory-2026"),
            },
            1
        )]
    );
    assert_contract(&harness.events);
}

#[tokio::test]
async fn refuses_a_capped_credential_while_its_installations_cannot_be_counted() {
    let harness = harness().await;
    harness.provisioning.replace_fleet_tokens(tokens_file(&[
        ("lab", "lab secret", "max_installations = 5"),
        ("open", "open secret", ""),
    ]));
    *harness.quota.unavailable.lock().unwrap() = true;
    let client = harness.client(ADDRESS, Vec::new());
    let refused = assign(&client, Credential::Fleet("lab secret"), &[40u8; 32])
        .await
        .err()
        .unwrap();
    assert_eq!(refused.kind, capnp::ErrorKind::Overloaded);
    let event = harness.events.last();
    assert_eq!(
        (event.outcome, event.reason.as_deref()),
        (Outcome::Error, Some("credential_quota_unavailable"))
    );
    enroll_new(&client, Credential::Fleet("open secret"), [41u8; 32])
        .await
        .unwrap();
    assert_contract(&harness.events);
}
