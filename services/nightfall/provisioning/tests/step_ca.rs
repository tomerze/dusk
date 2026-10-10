use std::collections::BTreeSet;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use dusk_capnp::capnp;
use nightfall_provisioning::config::{ProvisioningConfig, load_certificates};
use nightfall_provisioning::credential::{FleetTokens, InstallTokenKeys, sha256};
use nightfall_provisioning::events::{EnrollmentEvent, EnrollmentEvents, Outcome};
use nightfall_provisioning::identity::{
    DeviceIdKey, TPM_ATTESTATION_URI, device_uri, installation_uri, tenant_uri,
};
use nightfall_provisioning::limits::PenaltyBox;
use nightfall_provisioning::provision_capnp::provisioning;
use nightfall_provisioning::server::{ConnectionInfo, Provisioning};
use nightfall_provisioning::state::{Lifecycle, NodeStateView};
use nightfall_provisioning::step_ca::{SignRequest, StepCaClient, StepCaConfig, StepCaError};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair as _};
use serde_json::json;
use x509_parser::extensions::{GeneralName, ParsedExtension};

const LIFETIME: Duration = Duration::from_secs(168 * 3600);

struct Settings {
    url: String,
    root: PathBuf,
    provisioner_key: PathBuf,
}

fn settings() -> Option<Settings> {
    let variable = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    Some(Settings {
        url: variable("NIGHTFALL_STEP_CA_TEST_URL")?,
        root: PathBuf::from(variable("NIGHTFALL_STEP_CA_TEST_ROOT")?),
        provisioner_key: PathBuf::from(variable("NIGHTFALL_STEP_CA_TEST_PROVISIONER_KEY")?),
    })
}

#[derive(Default)]
struct Recorder(Mutex<Vec<EnrollmentEvent>>);

impl EnrollmentEvents for Recorder {
    fn record(&self, event: EnrollmentEvent) {
        self.0.lock().unwrap().push(event);
    }
}

struct Everyone;

impl NodeStateView for Everyone {
    fn device(&self, _device_id: &str) -> Option<Lifecycle> {
        None
    }

    fn installation(&self, _device_id: &str, _installation_id: &str) -> Option<Lifecycle> {
        None
    }
}

impl PenaltyBox for Everyone {
    fn penalized(&self, _address: IpAddr) -> bool {
        false
    }

    fn admit_enrollment(&self, _address: IpAddr) -> bool {
        true
    }

    fn credential_failed(&self, _address: IpAddr) {}
}

fn unique() -> String {
    let mut bytes = [0u8; 16];
    SystemRandom::new().fill(&mut bytes).unwrap();
    hex::encode(bytes)
}

fn step_ca_config(settings: &Settings) -> StepCaConfig {
    let mut config = StepCaConfig::load(
        &settings.url,
        &settings.root,
        "nightfall",
        &settings.provisioner_key,
        LIFETIME,
    )
    .unwrap();
    config.timeout = Duration::from_secs(10);
    config
}

fn csr(key: &KeyPair, common_name: Option<&str>, uris: &[String]) -> Vec<u8> {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    let mut name = DistinguishedName::new();
    if let Some(common_name) = common_name {
        name.push(DnType::CommonName, common_name);
    }
    params.distinguished_name = name;
    params.subject_alt_names = uris
        .iter()
        .map(|uri| SanType::URI(uri.as_str().try_into().unwrap()))
        .collect();
    params.serialize_request(key).unwrap().der().to_vec()
}

fn describe(der: &[u8]) -> (BTreeSet<String>, String, Vec<String>, i64) {
    let (_, certificate) = x509_parser::parse_x509_certificate(der).unwrap();
    let mut uris = BTreeSet::new();
    let mut usages = Vec::new();
    for extension in certificate.extensions() {
        match extension.parsed_extension() {
            ParsedExtension::SubjectAlternativeName(names) => {
                for name in &names.general_names {
                    match name {
                        GeneralName::URI(uri) => {
                            uris.insert(String::from(*uri));
                        }
                        other => panic!("unexpected SAN {other:?}"),
                    }
                }
            }
            ParsedExtension::ExtendedKeyUsage(usage) => {
                usages.push(format!(
                    "client_auth={} server_auth={}",
                    usage.client_auth, usage.server_auth
                ));
            }
            ParsedExtension::KeyUsage(usage) => usages.push(format!("{usage}")),
            _ => {}
        }
    }
    let common_name = certificate
        .subject()
        .iter_common_name()
        .next()
        .map(|name| String::from(name.as_str().unwrap()))
        .unwrap_or_default();
    let validity = certificate.validity();
    (
        uris,
        common_name,
        usages,
        validity.not_after.timestamp() - validity.not_before.timestamp(),
    )
}

async fn assign(
    client: &provisioning::Client,
    token: &str,
    install: bool,
    fingerprint: &[u8],
) -> Result<(String, String, Vec<u8>), capnp::Error> {
    let mut request = client.assign_request();
    if install {
        request.get().init_credential().set_install_token(token);
    } else {
        request.get().init_credential().set_fleet_token(token);
    }
    request
        .get()
        .init_device()
        .set_hardware_fingerprint(fingerprint);
    let response = request.send().promise.await?;
    let assignment = response.get()?.get_assignment()?;
    Ok((
        String::from(assignment.get_device_id()?.to_str()?),
        String::from(assignment.get_installation_id()?.to_str()?),
        assignment.get_challenge()?.to_vec(),
    ))
}

async fn enroll(
    client: &provisioning::Client,
    token: &str,
    install: bool,
    fingerprint: &[u8],
    challenge: &[u8],
    csr_der: &[u8],
) -> Result<Vec<Vec<u8>>, capnp::Error> {
    let mut request = client.enroll_request();
    if install {
        request.get().init_credential().set_install_token(token);
    } else {
        request.get().init_credential().set_fleet_token(token);
    }
    request
        .get()
        .init_device()
        .set_hardware_fingerprint(fingerprint);
    request.get().set_challenge(challenge);
    request.get().set_csr(csr_der);
    let response = request.send().promise.await?;
    response
        .get()?
        .get_issued()?
        .get_certificate_chain()?
        .iter()
        .map(|certificate| certificate.map(<[u8]>::to_vec))
        .collect()
}

#[tokio::test]
#[ignore = "needs a step-ca configured as the docs describe; see NIGHTFALL_STEP_CA_TEST_URL"]
async fn issues_node_certificates_through_a_real_step_ca() {
    let settings = settings()
        .expect("set NIGHTFALL_STEP_CA_TEST_URL, _ROOT and _PROVISIONER_KEY to run this test");
    let installer = Ed25519KeyPair::from_pkcs8(
        Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .unwrap()
            .as_ref(),
    )
    .unwrap();
    let installer_jwks = json!({ "keys": [{
        "kty": "OKP", "crv": "Ed25519", "kid": "factory-2026",
        "x": URL_SAFE_NO_PAD.encode(installer.public_key().as_ref()),
    }] });
    let roots = load_certificates(&settings.root).unwrap();
    let events = Arc::new(Recorder::default());
    let config = ProvisioningConfig {
        instance: String::from("nightfall-0"),
        fleet_tokens: FleetTokens::from_toml(&format!(
            "[[token]]\nname = \"retail-eu-2026\"\nvalue_sha256 = \"{}\"\ntenant = \"retail-eu\"\n",
            hex::encode(sha256(b"retail secret"))
        ))
        .unwrap(),
        install_token_keys: InstallTokenKeys::from_json(&installer_jwks.to_string()).unwrap(),
        device_id_key: DeviceIdKey::from_hex(&"5a".repeat(32)).unwrap(),
        fleet_client_roots: roots,
        challenge_ttl: Duration::from_secs(300),
        challenge_capacity: 100,
        renew_grace: Duration::from_secs(90 * 24 * 3600),
        certificate_lifetime: LIFETIME,
        enrollments_per_second: 100,
        enrollments_per_second_per_credential: 100,
        enrollment_alert_per_minute: 1000,
    };
    let provisioning = Provisioning::new(
        config,
        StepCaClient::new(step_ca_config(&settings)).unwrap(),
        events.clone(),
        Arc::new(Everyone),
        Arc::new(Everyone),
    )
    .unwrap();
    let client = provisioning.client(ConnectionInfo {
        remote_address: "198.51.100.7:40000".parse().unwrap(),
        peer_certificates: Vec::new(),
    });

    let fingerprint = sha256(b"machine-id of the step-ca test");
    let (device_id, installation_id, challenge) =
        assign(&client, "retail secret", false, &fingerprint)
            .await
            .unwrap();
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let uris = vec![device_uri(&device_id), installation_uri(&installation_id)];
    let chain = enroll(
        &client,
        "retail secret",
        false,
        &fingerprint,
        &challenge,
        &csr(&key, None, &uris),
    )
    .await
    .unwrap();
    assert_eq!(chain.len(), 2);
    let (names, common_name, usages, lifetime) = describe(&chain[0]);
    let mut expected: BTreeSet<String> = uris.iter().cloned().collect();
    expected.insert(tenant_uri("retail-eu"));
    assert_eq!(names, expected);
    assert_eq!(common_name, "");
    assert!(
        usages.contains(&String::from("client_auth=true server_auth=false")),
        "{usages:?}"
    );
    assert!(
        usages.contains(&String::from("Digital Signature")),
        "{usages:?}"
    );
    assert!(
        lifetime >= LIFETIME.as_secs() as i64 && lifetime <= LIFETIME.as_secs() as i64 + 120,
        "{lifetime}"
    );
    let trust = provisioning.trust();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let verified = trust
        .verify(
            &chain
                .iter()
                .map(|der| der.clone().into())
                .collect::<Vec<_>>(),
            now,
        )
        .unwrap();
    assert_eq!(verified.identity.tenant.as_deref(), Some("retail-eu"));

    let claims =
        json!({ "iss": "factory", "sub": "kiosk-0042", "jti": unique(), "exp": now + 3600 });
    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"EdDSA","kid":"factory-2026","typ":"JWT"}"#);
    let signed = format!("{header}.{}", URL_SAFE_NO_PAD.encode(claims.to_string()));
    let install_token = format!(
        "{signed}.{}",
        URL_SAFE_NO_PAD.encode(installer.sign(signed.as_bytes()).as_ref())
    );
    let install_fingerprint = sha256(b"another machine");
    let (install_device, assigned_installation, challenge) =
        assign(&client, &install_token, true, &install_fingerprint)
            .await
            .unwrap();
    assert_eq!(assigned_installation.len(), 32);
    let install_uris = vec![
        device_uri(&install_device),
        installation_uri(&assigned_installation),
    ];
    let named = format!("{install_device}.{assigned_installation}");
    let chain = enroll(
        &client,
        &install_token,
        true,
        &install_fingerprint,
        &challenge,
        &csr(&key, Some(&named), &install_uris),
    )
    .await
    .unwrap();
    let (names, common_name, _, _) = describe(&chain[0]);
    assert_eq!(names, install_uris.iter().cloned().collect::<BTreeSet<_>>());
    assert_eq!(common_name, "");
    let (_, again_installation, challenge) =
        assign(&client, &install_token, true, &install_fingerprint)
            .await
            .unwrap();
    assert_ne!(again_installation, assigned_installation);
    let reused = enroll(
        &client,
        &install_token,
        true,
        &install_fingerprint,
        &challenge,
        &csr(
            &key,
            None,
            &[
                device_uri(&install_device),
                installation_uri(&again_installation),
            ],
        ),
    )
    .await;
    let refusal = reused.err().unwrap();
    assert!(
        refusal.extra.contains("certificate authority refused"),
        "{}",
        refusal.extra
    );

    let outcomes: Vec<(Outcome, Option<String>)> = events
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|event| (event.outcome, event.reason.clone()))
        .collect();
    assert_eq!(
        outcomes,
        vec![
            (Outcome::Assigned, None),
            (Outcome::Issued, None),
            (Outcome::Assigned, None),
            (Outcome::Issued, None),
            (Outcome::Assigned, None),
            (Outcome::Denied, Some(String::from("step_ca_unauthorized"))),
        ]
    );

    let direct = StepCaClient::new(step_ca_config(&settings)).unwrap();
    let extra = format!("urn:dusk:device:{}", "1".repeat(32));
    let mut three = uris.clone();
    three.push(extra);
    let refused = direct
        .sign(
            &SignRequest {
                csr_der: csr(&key, None, &three),
                subject: format!("{device_id}.{installation_id}"),
                sans: three,
                tenant: None,
                token_id: unique(),
                tpm_bound: false,
            },
            now,
        )
        .await;
    assert!(
        matches!(refused, Err(StepCaError::Refused { status: 400, .. })),
        "{refused:?}"
    );
    let bad_tenant = direct
        .sign(
            &SignRequest {
                csr_der: csr(&key, None, &uris),
                subject: format!("{device_id}.{installation_id}"),
                sans: uris.clone(),
                tenant: Some(String::from("Not A Tenant")),
                token_id: unique(),
                tpm_bound: false,
            },
            now,
        )
        .await;
    assert!(
        matches!(bad_tenant, Err(StepCaError::Refused { status: 400, .. })),
        "{bad_tenant:?}"
    );
    let attested = direct
        .sign(
            &SignRequest {
                csr_der: csr(&key, None, &uris),
                subject: format!("{device_id}.{installation_id}"),
                sans: uris.clone(),
                tenant: None,
                token_id: unique(),
                tpm_bound: true,
            },
            now,
        )
        .await
        .unwrap();
    let (names, _, _, _) = describe(&attested.chain[0]);
    let mut expected: BTreeSet<String> = uris.iter().cloned().collect();
    expected.insert(String::from(TPM_ATTESTATION_URI));
    assert_eq!(names, expected);
}
