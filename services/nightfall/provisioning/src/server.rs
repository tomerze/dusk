use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::provision_capnp::{RENEW_BEYOND_GRACE, credential, device_report, provisioning};
use dusk_capnp::capnp;
use dusk_capnp::capnp::capability::Promise;
use ring::rand::{SecureRandom, SystemRandom};
use rustls_pki_types::{CertificateDer, TrustAnchor, UnixTime};
use tracing::{error, info, warn};

use crate::challenge::{Binding, ChallengeStore};
use crate::config::ProvisioningConfig;
use crate::credential::{CredentialKind, FleetTokens, InstallToken, InstallTokenKeys, sha256};
use crate::csr::validate_csr;
use crate::events::{
    EnrollmentEvent, EnrollmentEvents, Operation, Outcome, format_unix_seconds, remote_address,
    reported,
};
use crate::identity::{
    DeviceIdKey, HARDWARE_FINGERPRINT_BYTES, common_name, device_uri, installation_uri,
    new_installation_id,
};
use crate::limits::{CollisionTracker, EnrollmentBuckets, PenaltyBox, RateWindow, RenewLimiter};
use crate::renew::FleetClientTrust;
use crate::state::NodeStateView;
use crate::step_ca::{SignRequest, SignedCertificate, StepCaClient, StepCaError};
use crate::tpm::{self, Evidence};

pub const CREDENTIAL_BUCKETS: usize = 100_000;
pub const RENEW_IDENTITIES: usize = 262_144;
pub const COLLISION_DEVICES: usize = 100_000;
const LIFETIME_TOLERANCE: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub remote_address: SocketAddr,
    pub peer_certificates: Vec<CertificateDer<'static>>,
}

struct Refusal {
    outcome: Outcome,
    reason: &'static str,
    detail: String,
    error: capnp::Error,
}

impl Refusal {
    fn denied(reason: &'static str, detail: impl Into<String>, message: &str) -> Refusal {
        Refusal {
            outcome: Outcome::Denied,
            reason,
            detail: detail.into(),
            error: capnp::Error::failed(format!("denied: {message}")),
        }
    }

    fn rate_limited(reason: &'static str, detail: impl Into<String>) -> Refusal {
        Refusal {
            outcome: Outcome::RateLimited,
            reason,
            detail: detail.into(),
            error: capnp::Error::overloaded(String::from("rate limited; retry later")),
        }
    }

    fn failure(reason: &'static str, detail: impl Into<String>, error: capnp::Error) -> Refusal {
        Refusal {
            outcome: Outcome::Error,
            reason,
            detail: detail.into(),
            error,
        }
    }
}

enum Presented {
    FleetToken(String),
    InstallToken(String),
    Certificate,
}

impl Presented {
    fn read(
        reader: credential::Reader<'_>,
        kind: &mut CredentialKind,
    ) -> Result<Presented, capnp::Error> {
        Ok(match reader.which()? {
            credential::FleetToken(token) => {
                *kind = CredentialKind::FleetToken;
                Presented::FleetToken(String::from(token?.to_str()?))
            }
            credential::InstallToken(token) => {
                *kind = CredentialKind::InstallToken;
                Presented::InstallToken(String::from(token?.to_str()?))
            }
            credential::Certificate(()) => {
                *kind = CredentialKind::Certificate;
                Presented::Certificate
            }
        })
    }

    fn secret(&self) -> &[u8] {
        match self {
            Presented::FleetToken(token) | Presented::InstallToken(token) => token.as_bytes(),
            Presented::Certificate => &[],
        }
    }
}

struct Verified {
    reference: String,
    tenant: Option<String>,
    install_token: Option<InstallToken>,
}

struct Report {
    fingerprint: Vec<u8>,
    installation_hint: Option<String>,
    dusk_version: Option<String>,
    implementation: Option<String>,
    target_os: Option<String>,
    target_arch: Option<String>,
    hostname: Option<String>,
    tpm: Option<Evidence>,
}

impl Report {
    fn read(reader: device_report::Reader<'_>) -> Result<Report, capnp::Error> {
        let text = |field: capnp::Result<capnp::text::Reader<'_>>| -> Option<String> {
            field
                .ok()
                .and_then(|text| text.to_str().ok())
                .and_then(reported)
        };
        Ok(Report {
            fingerprint: reader.get_hardware_fingerprint()?.to_vec(),
            installation_hint: text(reader.get_installation_hint()),
            dusk_version: text(reader.get_dusk_version()),
            implementation: text(reader.get_impl()),
            target_os: text(reader.get_target_os()),
            target_arch: text(reader.get_target_arch()),
            hostname: text(reader.get_hostname()),
            tpm: if reader.has_tpm() {
                Some(Evidence::read(reader.get_tpm()?)?)
            } else {
                None
            },
        })
    }

    fn endorsement_key(&self) -> Option<&[u8]> {
        self.tpm
            .as_ref()
            .map(|evidence| evidence.endorsement_key.as_slice())
    }
}

struct Attempt {
    event: EnrollmentEvent,
    tpm_bound: bool,
}

impl Attempt {
    fn new(
        operation: Operation,
        kind: CredentialKind,
        connection: &ConnectionInfo,
        instance: &str,
    ) -> Attempt {
        Attempt {
            event: EnrollmentEvent {
                operation,
                outcome: Outcome::Error,
                reason: None,
                device_id: None,
                installation_id: None,
                tenant: None,
                credential_kind: kind,
                credential_ref: None,
                credential_issuer: None,
                hardware_fingerprint_hash: None,
                remote_address: remote_address(connection.remote_address),
                cert_serial: None,
                cert_fingerprint: None,
                cert_not_after: None,
                dusk_version: None,
                implementation: None,
                target_os: None,
                target_arch: None,
                hostname: None,
                instance: String::from(instance),
            },
            tpm_bound: false,
        }
    }

    fn report(&mut self, report: &Report) {
        self.event.hardware_fingerprint_hash = Some(hex::encode(sha256(&report.fingerprint)));
        self.event.dusk_version = report.dusk_version.clone();
        self.event.implementation = report.implementation.clone();
        self.event.target_os = report.target_os.clone();
        self.event.target_arch = report.target_arch.clone();
        self.event.hostname = report.hostname.clone();
    }

    fn identify(&mut self, device_id: &str, installation_id: &str) {
        self.event.device_id = Some(String::from(device_id));
        self.event.installation_id = Some(String::from(installation_id));
    }

    fn issued(&mut self, signed: &SignedCertificate) {
        self.event.cert_serial = Some(signed.serial.clone());
        self.event.cert_fingerprint = Some(signed.fingerprint.clone());
        self.event.cert_not_after = format_unix_seconds(signed.not_after_unix);
    }
}

struct Assigned {
    device_id: String,
    installation_id: String,
    challenge: [u8; 32],
    expires_unix_ms: u64,
    credential: Option<(Vec<u8>, Vec<u8>)>,
}

pub struct Provisioning {
    instance: String,
    fleet_tokens: RwLock<Arc<FleetTokens>>,
    install_token_keys: RwLock<Arc<InstallTokenKeys>>,
    device_id_key: DeviceIdKey,
    endorsement_roots: Vec<TrustAnchor<'static>>,
    renew_grace: Duration,
    certificate_lifetime: Duration,
    enrollment_alert_per_minute: u64,
    step_ca: StepCaClient,
    events: Arc<dyn EnrollmentEvents>,
    state: Arc<dyn NodeStateView>,
    penalty_box: Arc<dyn PenaltyBox>,
    trust: Arc<FleetClientTrust>,
    challenges: ChallengeStore,
    buckets: Mutex<EnrollmentBuckets>,
    renewals: Mutex<RenewLimiter>,
    rate: Mutex<(RateWindow, bool)>,
    collisions: Mutex<CollisionTracker>,
    random: SystemRandom,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn current<T>(shared: &RwLock<Arc<T>>) -> Arc<T> {
    shared
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

fn replace<T>(shared: &RwLock<Arc<T>>, value: T) -> Arc<T> {
    std::mem::replace(
        &mut *shared
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
        Arc::new(value),
    )
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn random_hex(random: &SystemRandom, bytes: usize) -> Result<String, Refusal> {
    let mut buffer = vec![0u8; bytes];
    random.fill(&mut buffer).map_err(|_| {
        Refusal::failure(
            "random_source",
            "the system random source failed",
            capnp::Error::failed(String::from("internal error")),
        )
    })?;
    Ok(hex::encode(buffer))
}

impl Provisioning {
    pub fn new(
        config: ProvisioningConfig,
        step_ca: StepCaClient,
        events: Arc<dyn EnrollmentEvents>,
        state: Arc<dyn NodeStateView>,
        penalty_box: Arc<dyn PenaltyBox>,
    ) -> Result<Arc<Provisioning>, String> {
        let trust = Arc::new(FleetClientTrust::new(&config.fleet_client_roots)?);
        let endorsement_roots = config
            .endorsement_roots
            .iter()
            .map(|root| {
                webpki::anchor_from_trusted_cert(root)
                    .map(|anchor| anchor.to_owned())
                    .map_err(|error| {
                        format!("a TPM endorsement root is not a usable trust anchor: {error}")
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if step_ca.certificate_lifetime() != config.certificate_lifetime {
            return Err(format!(
                "the step-ca client asks for {:?} certificates but provisioning expects {:?}",
                step_ca.certificate_lifetime(),
                config.certificate_lifetime
            ));
        }
        let now = Instant::now();
        info!(
            instance = %config.instance,
            fleet_tokens = config.fleet_tokens.len(),
            retired_fleet_tokens = config.fleet_tokens.retired(),
            install_token_keys = config.install_token_keys.len(),
            tpm_endorsement_roots = endorsement_roots.len(),
            "provisioning ready"
        );
        Ok(Arc::new(Provisioning {
            instance: config.instance,
            fleet_tokens: RwLock::new(Arc::new(config.fleet_tokens)),
            install_token_keys: RwLock::new(Arc::new(config.install_token_keys)),
            device_id_key: config.device_id_key,
            endorsement_roots,
            renew_grace: config.renew_grace,
            certificate_lifetime: config.certificate_lifetime,
            enrollment_alert_per_minute: config.enrollment_alert_per_minute,
            step_ca,
            events,
            state,
            penalty_box,
            trust,
            challenges: ChallengeStore::new(config.challenge_capacity, config.challenge_ttl),
            buckets: Mutex::new(EnrollmentBuckets::new(
                config.enrollments_per_second,
                config.enrollments_per_second_per_credential,
                CREDENTIAL_BUCKETS,
                now,
            )),
            renewals: Mutex::new(RenewLimiter::new(RENEW_IDENTITIES)),
            rate: Mutex::new((RateWindow::new(now), false)),
            collisions: Mutex::new(CollisionTracker::new(COLLISION_DEVICES)),
            random: SystemRandom::new(),
        }))
    }

    pub fn trust(&self) -> Arc<FleetClientTrust> {
        self.trust.clone()
    }

    pub fn replace_fleet_tokens(&self, tokens: FleetTokens) {
        let named =
            |tokens: &FleetTokens| -> std::collections::BTreeMap<String, (Option<String>, bool)> {
                tokens
                    .iter()
                    .map(|token| (token.name.clone(), (token.tenant.clone(), token.retired)))
                    .collect()
            };
        let after = named(&tokens);
        let count = tokens.len();
        let retired_count = tokens.retired();
        let before = named(&replace(&self.fleet_tokens, tokens));
        let added: Vec<&String> = after
            .keys()
            .filter(|name| !before.contains_key(*name))
            .collect();
        let removed: Vec<&String> = before
            .keys()
            .filter(|name| !after.contains_key(*name))
            .collect();
        let retired: Vec<&String> = after
            .iter()
            .filter(|(name, entry)| entry.1 && before.get(*name).is_none_or(|previous| !previous.1))
            .map(|(name, _)| name)
            .collect();
        let changed: Vec<&String> = after
            .iter()
            .filter(|(name, entry)| before.get(*name).is_some_and(|previous| previous != *entry))
            .map(|(name, _)| name)
            .collect();
        info!(
            fleet_tokens = count,
            retired_fleet_tokens = retired_count,
            ?added,
            ?removed,
            ?retired,
            ?changed,
            "fleet tokens reloaded"
        );
    }

    pub fn replace_install_token_keys(&self, keys: InstallTokenKeys) {
        let count = keys.len();
        replace(&self.install_token_keys, keys);
        info!(install_token_keys = count, "install token keys reloaded");
    }

    pub fn client(self: &Arc<Self>, connection: ConnectionInfo) -> provisioning::Client {
        dusk_capnp::capnp_rpc::new_client(ProvisioningServer {
            shared: self.clone(),
            connection: Rc::new(connection),
        })
    }

    fn finish(&self, mut attempt: Attempt, result: Result<(), &Refusal>) {
        if let Err(refusal) = result {
            attempt.event.outcome = refusal.outcome;
            attempt.event.reason = Some(String::from(refusal.reason));
        }
        let event = &attempt.event;
        let operation = event.operation.name();
        let outcome = event.outcome.name();
        let detail = result
            .err()
            .map(|refusal| refusal.detail.as_str())
            .unwrap_or("");
        match event.outcome {
            Outcome::Issued | Outcome::Assigned => info!(
                operation, outcome, device_id = event.device_id.as_deref(), installation_id = event.installation_id.as_deref(),
                tenant = event.tenant.as_deref(), credential_ref = event.credential_ref.as_deref(), tpm_bound = attempt.tpm_bound,
                remote_address = %event.remote_address, cert_serial = event.cert_serial.as_deref(), "provisioning request succeeded"
            ),
            Outcome::Denied | Outcome::RateLimited => warn!(
                operation, outcome, reason = event.reason.as_deref(), detail, device_id = event.device_id.as_deref(),
                installation_id = event.installation_id.as_deref(), credential_ref = event.credential_ref.as_deref(),
                remote_address = %event.remote_address, "provisioning request refused"
            ),
            Outcome::Error => error!(
                operation, outcome, reason = event.reason.as_deref(), detail, device_id = event.device_id.as_deref(),
                installation_id = event.installation_id.as_deref(), remote_address = %event.remote_address,
                "provisioning request failed"
            ),
        }
        if event.operation != Operation::Renew {
            self.count_enrollment();
        }
        self.events.record(attempt.event);
    }

    fn count_enrollment(&self) {
        let mut guard = lock(&self.rate);
        let (window, active) = &mut *guard;
        let per_minute = window.record(Instant::now());
        self.update_rate_alert(active, per_minute);
    }

    pub fn refresh_rate_alert(&self, now: Instant) {
        let mut guard = lock(&self.rate);
        let (window, active) = &mut *guard;
        let per_minute = window.count(now);
        self.update_rate_alert(active, per_minute);
    }

    fn update_rate_alert(&self, active: &mut bool, per_minute: u64) {
        let above = per_minute > self.enrollment_alert_per_minute;
        if above != *active {
            *active = above;
            if above {
                warn!(
                    per_minute,
                    threshold = self.enrollment_alert_per_minute,
                    "enrollment rate above the alert threshold"
                );
            } else {
                info!(
                    per_minute,
                    threshold = self.enrollment_alert_per_minute,
                    "enrollment rate back below the alert threshold"
                );
            }
            self.events.rate_alert(per_minute, above);
        }
    }

    fn refuse_malformed(
        &self,
        connection: &ConnectionInfo,
        mut attempt: Attempt,
        error: &capnp::Error,
    ) -> capnp::Error {
        if attempt.event.operation != Operation::Renew {
            attempt.event.hardware_fingerprint_hash = Some(hex::encode(sha256(&[])));
        }
        self.penalty_box
            .credential_failed(connection.remote_address.ip());
        let refusal = Refusal::denied("malformed_request", error.to_string(), "malformed request");
        self.finish(attempt, Err(&refusal));
        refusal.error
    }

    fn admit(
        &self,
        connection: &ConnectionInfo,
        presented: &Presented,
        endorsement_key: Option<&[u8]>,
        operation: Operation,
    ) -> Result<(), Refusal> {
        let address = connection.remote_address.ip();
        if self.penalty_box.penalized(address) {
            return Err(Refusal::rate_limited(
                "penalty_box",
                format!("{address} is in the penalty box"),
            ));
        }
        if operation == Operation::Assign && !self.penalty_box.admit_enrollment(address) {
            return Err(Refusal::rate_limited(
                "network_enrollment_rate",
                format!("the enrollments per hour of the network of {address} are used up"),
            ));
        }
        let bucket = hex::encode(sha256(presented.secret()));
        let endorsement_bucket =
            endorsement_key.map(|key| format!("tpm:{}", hex::encode(sha256(key))));
        lock(&self.buckets)
            .admit(&bucket, endorsement_bucket.as_deref(), Instant::now())
            .map_err(|limited| {
                Refusal::rate_limited(
                    match limited {
                        crate::limits::RateLimited::Instance => "enrollment_rate",
                        crate::limits::RateLimited::Credential => "credential_rate",
                        crate::limits::RateLimited::EndorsementKey => "endorsement_key_rate",
                    },
                    limited.to_string(),
                )
            })
    }

    fn verify_credential(
        &self,
        connection: &ConnectionInfo,
        presented: &Presented,
        attempt: &mut Attempt,
    ) -> Result<Verified, Refusal> {
        let address = connection.remote_address.ip();
        let verified = match presented {
            Presented::FleetToken(token) => match current(&self.fleet_tokens).find(token) {
                Some(entry) if entry.retired => {
                    attempt.event.credential_ref = Some(entry.name.clone());
                    attempt.event.tenant = entry.tenant.clone();
                    return Err(Refusal::denied(
                        "credential_retired",
                        format!("fleet token {} is retired", entry.name),
                        "credential retired",
                    ));
                }
                Some(entry) => Verified {
                    reference: entry.name.clone(),
                    tenant: entry.tenant.clone(),
                    install_token: None,
                },
                None => {
                    self.penalty_box.credential_failed(address);
                    return Err(Refusal::denied(
                        "invalid_credential",
                        "no fleet token entry matches",
                        "invalid credential",
                    ));
                }
            },
            Presented::InstallToken(token) => {
                match current(&self.install_token_keys).verify(token, unix_now()) {
                    Ok(install_token) => Verified {
                        reference: install_token.subject.clone(),
                        tenant: install_token.tenant.clone(),
                        install_token: Some(install_token),
                    },
                    Err(failure) => {
                        self.penalty_box.credential_failed(address);
                        return Err(Refusal::denied(
                            "invalid_install_token",
                            failure.to_string(),
                            "invalid credential",
                        ));
                    }
                }
            }
            Presented::Certificate => {
                return Err(Refusal::denied(
                    "certificate_credential",
                    "assign and enroll need a fleet token or an install token",
                    "a certificate is accepted by renew only",
                ));
            }
        };
        attempt.event.credential_ref = Some(verified.reference.clone());
        attempt.event.credential_issuer = verified
            .install_token
            .as_ref()
            .map(|install_token| install_token.key_id.clone());
        attempt.event.tenant = verified.tenant.clone();
        Ok(verified)
    }

    fn check_fingerprint(&self, report: &Report) -> Result<String, Refusal> {
        if report.fingerprint.len() != HARDWARE_FINGERPRINT_BYTES {
            return Err(Refusal::denied(
                "invalid_hardware_fingerprint",
                format!(
                    "the hardware fingerprint is {} bytes",
                    report.fingerprint.len()
                ),
                "the hardware fingerprint must be 32 bytes",
            ));
        }
        Ok(self.device_id_key.device_id(&report.fingerprint))
    }

    fn check_state(&self, device_id: &str, installation_id: &str) -> Result<(), Refusal> {
        if let Some(lifecycle) = self.state.device(device_id)
            && lifecycle.blocks_certificates()
        {
            let reason = if lifecycle == crate::state::Lifecycle::Revoked {
                "device_revoked"
            } else {
                "device_retired"
            };
            return Err(Refusal::denied(
                reason,
                format!("device {device_id} is {}", lifecycle.name()),
                &format!("device {}", lifecycle.name()),
            ));
        }
        if let Some(lifecycle) = self.state.installation(device_id, installation_id)
            && lifecycle.blocks_certificates()
        {
            let reason = if lifecycle == crate::state::Lifecycle::Revoked {
                "installation_revoked"
            } else {
                "installation_retired"
            };
            return Err(Refusal::denied(
                reason,
                format!("installation {installation_id} is {}", lifecycle.name()),
                &format!("installation {}", lifecycle.name()),
            ));
        }
        Ok(())
    }

    fn attest(&self, evidence: &Evidence, report: &Report) -> Result<tpm::Attested, Refusal> {
        tpm::attest(
            evidence,
            &report.fingerprint,
            &self.endorsement_roots,
            UnixTime::now(),
        )
        .map_err(|failure| {
            Refusal::denied(failure.reason(), failure.to_string(), &failure.to_string())
        })
    }

    fn assign(
        &self,
        connection: &ConnectionInfo,
        presented: &Presented,
        report: &Report,
        attempt: &mut Attempt,
    ) -> Result<Assigned, Refusal> {
        self.admit(
            connection,
            presented,
            report.endorsement_key(),
            Operation::Assign,
        )?;
        self.verify_credential(connection, presented, attempt)?;
        let attested = report
            .tpm
            .as_ref()
            .map(|evidence| self.attest(evidence, report))
            .transpose()?;
        let device_id = self.check_fingerprint(report)?;
        let installation_id = new_installation_id(&self.random).map_err(|_| {
            Refusal::failure(
                "random_source",
                "the system random source failed",
                capnp::Error::failed(String::from("internal error")),
            )
        })?;
        attempt.identify(&device_id, &installation_id);
        self.check_state(&device_id, &installation_id)?;
        let mut challenge = [0u8; 32];
        self.random.fill(&mut challenge).map_err(|_| {
            Refusal::failure(
                "random_source",
                "the system random source failed",
                capnp::Error::failed(String::from("internal error")),
            )
        })?;
        let credential = match &attested {
            Some(attested) => {
                let mut seed = [0u8; tpm::SEED_BYTES];
                self.random.fill(&mut seed).map_err(|_| {
                    Refusal::failure(
                        "random_source",
                        "the system random source failed",
                        capnp::Error::failed(String::from("internal error")),
                    )
                })?;
                Some(
                    tpm::make_credential(
                        &attested.endorsement_modulus,
                        &attested.node_key_name,
                        &challenge,
                        &seed,
                    )
                    .map_err(|error| {
                        Refusal::failure(
                            "tpm_credential",
                            format!("making the TPM credential failed: {error}"),
                            capnp::Error::failed(String::from("internal error")),
                        )
                    })?,
                )
            }
            None => None,
        };
        let binding = Binding {
            credential_digest: sha256(presented.secret()),
            fingerprint_digest: sha256(&report.fingerprint),
            device_id: device_id.clone(),
            installation_id: installation_id.clone(),
            node_key: attested.map(|attested| attested.node_key_public_key_info),
        };
        self.challenges
            .issue(challenge, binding, Instant::now())
            .map_err(|full| {
                Refusal::failure(
                    "challenge_store_full",
                    full.to_string(),
                    capnp::Error::overloaded(String::from(
                        "too many enrollments in progress; retry later",
                    )),
                )
            })?;
        let expires = SystemTime::now() + self.challenges.ttl();
        let expires_unix_ms = expires
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        if let Some(hint) = &report.installation_hint {
            info!(device_id = %device_id, installation_id = %installation_id, installation_hint = %hint, "node reported a previous installation");
        }
        attempt.tpm_bound = credential.is_some();
        Ok(Assigned {
            device_id,
            installation_id,
            challenge,
            expires_unix_ms,
            credential,
        })
    }

    fn prepare_enroll(
        &self,
        connection: &ConnectionInfo,
        presented: &Presented,
        report: &Report,
        challenge: &[u8],
        csr: &[u8],
        attempt: &mut Attempt,
    ) -> Result<SignRequest, Refusal> {
        self.admit(
            connection,
            presented,
            report.endorsement_key(),
            Operation::Enroll,
        )?;
        let verified = self.verify_credential(connection, presented, attempt)?;
        let device_id = self.check_fingerprint(report)?;
        let binding = self
            .challenges
            .take(challenge, Instant::now())
            .ok_or_else(|| {
                Refusal::denied(
                    "invalid_challenge",
                    "the challenge is unknown, used or expired",
                    "invalid challenge; assign again",
                )
            })?;
        if binding.device_id != device_id
            || binding.credential_digest != sha256(presented.secret())
            || binding.fingerprint_digest != sha256(&report.fingerprint)
        {
            return Err(Refusal::denied(
                "challenge_mismatch",
                "the challenge was issued for another credential or device",
                "invalid challenge; assign again",
            ));
        }
        let installation_id = binding.installation_id;
        attempt.identify(&device_id, &installation_id);
        self.check_state(&device_id, &installation_id)?;
        let validated = validate_csr(csr, &device_id, &installation_id).map_err(|failure| {
            Refusal::denied("invalid_csr", failure.to_string(), &failure.to_string())
        })?;
        if let Some(node_key) = &binding.node_key
            && validated.public_key != *node_key
        {
            return Err(Refusal::denied(
                "tpm_key_mismatch",
                "the CSR carries another key than the attested TPM key",
                "the CSR key is not the attested TPM key",
            ));
        }
        attempt.tpm_bound = binding.node_key.is_some();
        let token_id = match &verified.install_token {
            Some(install_token) => install_token.one_time_token_id(),
            None => random_hex(&self.random, 32)?,
        };
        Ok(SignRequest {
            csr_der: csr.to_vec(),
            subject: validated
                .common_name
                .unwrap_or_else(|| common_name(&device_id, &installation_id)),
            sans: vec![device_uri(&device_id), installation_uri(&installation_id)],
            tenant: verified.tenant,
            token_id,
            tpm_bound: attempt.tpm_bound,
        })
    }

    fn prepare_renew(
        &self,
        connection: &ConnectionInfo,
        csr: &[u8],
        attempt: &mut Attempt,
    ) -> Result<SignRequest, Refusal> {
        let now = unix_now();
        let certificate = self
            .trust
            .verify(&connection.peer_certificates, now)
            .map_err(|failure| {
                Refusal::denied(
                    "invalid_certificate",
                    failure.to_string(),
                    "renew needs the current fleet certificate as the TLS client certificate",
                )
            })?;
        let identity = &certificate.identity;
        attempt.identify(&identity.device_id, &identity.installation_id);
        attempt.event.tenant = identity.tenant.clone();
        let now_signed = i64::try_from(now).unwrap_or(i64::MAX);
        let grace_end = certificate
            .not_after_unix
            .saturating_add(i64::try_from(self.renew_grace.as_secs()).unwrap_or(i64::MAX));
        if now_signed > grace_end {
            return Err(Refusal {
                outcome: Outcome::Denied,
                reason: "beyond_renew_grace",
                detail: format!("the certificate expired at {}", certificate.not_after_unix),
                error: capnp::Error::failed(String::from(RENEW_BEYOND_GRACE)),
            });
        }
        self.check_state(&identity.device_id, &identity.installation_id)?;
        let half_life = certificate.not_before_unix
            + (certificate.not_after_unix - certificate.not_before_unix) / 2;
        if now_signed < half_life {
            return Err(Refusal::rate_limited(
                "renew_too_early",
                format!("renew is accepted from {half_life}"),
            ));
        }
        let validated = validate_csr(csr, &identity.device_id, &identity.installation_id).map_err(
            |failure| Refusal::denied("invalid_csr", failure.to_string(), &failure.to_string()),
        )?;
        if identity.tpm_bound && validated.public_key != certificate.public_key {
            return Err(Refusal::denied(
                "renew_key_changed",
                "the CSR carries another key than the TPM-bound certificate",
                "renew of a TPM-bound certificate needs the key it already has",
            ));
        }
        if !identity.tpm_bound && validated.public_key == certificate.public_key {
            return Err(Refusal::denied(
                "renew_key_reused",
                "the CSR carries the key of the current certificate",
                "renew needs a new key",
            ));
        }
        attempt.tpm_bound = identity.tpm_bound;
        lock(&self.renewals)
            .admit(
                &identity.device_id,
                &identity.installation_id,
                Instant::now(),
            )
            .map_err(|limited| Refusal::rate_limited("renew_limit", limited.to_string()))?;
        Ok(SignRequest {
            csr_der: csr.to_vec(),
            subject: validated
                .common_name
                .unwrap_or_else(|| common_name(&identity.device_id, &identity.installation_id)),
            sans: vec![
                device_uri(&identity.device_id),
                installation_uri(&identity.installation_id),
            ],
            tenant: identity.tenant.clone(),
            token_id: random_hex(&self.random, 32)?,
            tpm_bound: identity.tpm_bound,
        })
    }

    async fn sign(
        &self,
        request: &SignRequest,
        attempt: &mut Attempt,
    ) -> Result<SignedCertificate, Refusal> {
        let requested_at = unix_now();
        let signed =
            self.step_ca
                .sign(request, requested_at)
                .await
                .map_err(|failure| match failure {
                    StepCaError::Overloaded => Refusal::failure(
                        "step_ca_saturated",
                        failure.to_string(),
                        capnp::Error::overloaded(String::from(
                            "the certificate authority is busy; retry later",
                        )),
                    ),
                    StepCaError::Unauthorized { .. }
                        if attempt.event.credential_kind == CredentialKind::InstallToken =>
                    {
                        Refusal::denied(
                            "step_ca_unauthorized",
                            failure.to_string(),
                            "the certificate authority refused the request",
                        )
                    }
                    StepCaError::Unauthorized { .. } => Refusal::failure(
                        "step_ca_unauthorized",
                        failure.to_string(),
                        capnp::Error::failed(String::from(
                            "the certificate authority refused the request",
                        )),
                    ),
                    StepCaError::Refused { .. } => Refusal::failure(
                        "step_ca_refused",
                        failure.to_string(),
                        capnp::Error::failed(String::from(
                            "the certificate authority refused the request",
                        )),
                    ),
                    _ => Refusal::failure(
                        "step_ca_error",
                        failure.to_string(),
                        capnp::Error::failed(String::from(
                            "the certificate authority is unavailable; retry later",
                        )),
                    ),
                })?;
        let expected_end = requested_at.saturating_add(self.certificate_lifetime.as_secs()) as i64;
        if signed.not_after_unix.abs_diff(expected_end) > LIFETIME_TOLERANCE.as_secs() {
            warn!(
                serial = %signed.serial,
                not_after = signed.not_after_unix,
                expected_not_after = expected_end,
                "step-ca issued a certificate whose lifetime differs from certificate_lifetime"
            );
        }
        attempt.issued(&signed);
        Ok(signed)
    }

    fn renew_after(&self, signed: &SignedCertificate) -> u64 {
        let mut bytes = [0u8; 8];
        let fraction = match self.random.fill(&mut bytes) {
            Ok(()) => 0.55 + 0.2 * (u64::from_le_bytes(bytes) as f64 / u64::MAX as f64),
            Err(_) => 0.65,
        };
        let lifetime_ms = (signed.not_after_unix - signed.not_before_unix).max(0) as f64 * 1000.0;
        (signed.not_before_unix.max(0) as f64 * 1000.0 + lifetime_ms * fraction) as u64
    }

    fn record_collision(
        &self,
        connection: &ConnectionInfo,
        device_id: &str,
        installation_id: &str,
    ) {
        let collision = lock(&self.collisions).record(
            device_id,
            installation_id,
            connection.remote_address.ip(),
            Instant::now(),
        );
        if let Some(collision) = collision {
            warn!(
                device_id,
                installations = collision.installations,
                addresses = collision.addresses,
                "device id enrolled many installations from many addresses within 24 hours; the image may be cloned"
            );
            self.events.device_id_collision(
                device_id,
                collision.installations,
                collision.addresses,
            );
        }
    }
}

fn write_issued(
    mut issued: crate::provision_capnp::issued::Builder<'_>,
    signed: &SignedCertificate,
    renew_after_unix_ms: u64,
) {
    let mut chain = issued
        .reborrow()
        .init_certificate_chain(signed.chain.len() as u32);
    for (index, certificate) in signed.chain.iter().enumerate() {
        chain.set(index as u32, certificate.as_ref());
    }
    issued.set_not_after_unix_ms((signed.not_after_unix.max(0) as u64).saturating_mul(1000));
    issued.set_renew_after_unix_ms(renew_after_unix_ms);
}

struct ProvisioningServer {
    shared: Arc<Provisioning>,
    connection: Rc<ConnectionInfo>,
}

impl provisioning::Server for ProvisioningServer {
    fn assign(
        &mut self,
        params: provisioning::AssignParams,
        mut results: provisioning::AssignResults,
    ) -> Promise<(), capnp::Error> {
        let shared = &self.shared;
        let connection = &self.connection;
        let mut attempt = Attempt::new(
            Operation::Assign,
            CredentialKind::FleetToken,
            connection,
            &shared.instance,
        );
        let read = params.get().and_then(|request| {
            let presented = Presented::read(
                request.get_credential()?,
                &mut attempt.event.credential_kind,
            )?;
            Ok((presented, Report::read(request.get_device()?)?))
        });
        let (presented, report) = match read {
            Ok(read) => read,
            Err(error) => {
                return Promise::err(shared.refuse_malformed(connection, attempt, &error));
            }
        };
        attempt.report(&report);
        match shared.assign(connection, &presented, &report, &mut attempt) {
            Ok(assigned) => {
                let mut assignment = results.get().init_assignment();
                assignment.set_device_id(assigned.device_id.as_str());
                assignment.set_installation_id(assigned.installation_id.as_str());
                match &assigned.credential {
                    Some((credential_blob, encrypted_secret)) => {
                        assignment.set_credential_blob(credential_blob);
                        assignment.set_encrypted_secret(encrypted_secret);
                    }
                    None => assignment.set_challenge(&assigned.challenge),
                }
                assignment.set_challenge_expires_unix_ms(assigned.expires_unix_ms);
                attempt.event.outcome = Outcome::Assigned;
                shared.finish(attempt, Ok(()));
                Promise::ok(())
            }
            Err(refusal) => {
                shared.finish(attempt, Err(&refusal));
                Promise::err(refusal.error)
            }
        }
    }

    fn enroll(
        &mut self,
        params: provisioning::EnrollParams,
        mut results: provisioning::EnrollResults,
    ) -> Promise<(), capnp::Error> {
        let shared = self.shared.clone();
        let connection = self.connection.clone();
        let mut attempt = Attempt::new(
            Operation::Enroll,
            CredentialKind::FleetToken,
            &connection,
            &shared.instance,
        );
        let read = params.get().and_then(|request| {
            let presented = Presented::read(
                request.get_credential()?,
                &mut attempt.event.credential_kind,
            )?;
            Ok((
                presented,
                Report::read(request.get_device()?)?,
                request.get_challenge()?.to_vec(),
                request.get_csr()?.to_vec(),
            ))
        });
        let (presented, report, challenge, csr) = match read {
            Ok(read) => read,
            Err(error) => {
                return Promise::err(shared.refuse_malformed(&connection, attempt, &error));
            }
        };
        attempt.report(&report);
        let prepared = shared.prepare_enroll(
            &connection,
            &presented,
            &report,
            &challenge,
            &csr,
            &mut attempt,
        );
        Promise::from_future(async move {
            let request = match prepared {
                Ok(request) => request,
                Err(refusal) => {
                    shared.finish(attempt, Err(&refusal));
                    return Err(refusal.error);
                }
            };
            match shared.sign(&request, &mut attempt).await {
                Ok(signed) => {
                    write_issued(
                        results.get().init_issued(),
                        &signed,
                        shared.renew_after(&signed),
                    );
                    attempt.event.outcome = Outcome::Issued;
                    if let (Some(device_id), Some(installation_id)) = (
                        attempt.event.device_id.clone(),
                        attempt.event.installation_id.clone(),
                    ) {
                        shared.record_collision(&connection, &device_id, &installation_id);
                    }
                    shared.finish(attempt, Ok(()));
                    Ok(())
                }
                Err(refusal) => {
                    shared.finish(attempt, Err(&refusal));
                    Err(refusal.error)
                }
            }
        })
    }

    fn renew(
        &mut self,
        params: provisioning::RenewParams,
        mut results: provisioning::RenewResults,
    ) -> Promise<(), capnp::Error> {
        let shared = self.shared.clone();
        let connection = self.connection.clone();
        let mut attempt = Attempt::new(
            Operation::Renew,
            CredentialKind::Certificate,
            &connection,
            &shared.instance,
        );
        let csr = match params.get().and_then(|request| request.get_csr()) {
            Ok(csr) => csr.to_vec(),
            Err(error) => {
                return Promise::err(shared.refuse_malformed(&connection, attempt, &error));
            }
        };
        let prepared = shared.prepare_renew(&connection, &csr, &mut attempt);
        Promise::from_future(async move {
            let request = match prepared {
                Ok(request) => request,
                Err(refusal) => {
                    shared.finish(attempt, Err(&refusal));
                    return Err(refusal.error);
                }
            };
            match shared.sign(&request, &mut attempt).await {
                Ok(signed) => {
                    write_issued(
                        results.get().init_issued(),
                        &signed,
                        shared.renew_after(&signed),
                    );
                    attempt.event.outcome = Outcome::Issued;
                    shared.finish(attempt, Ok(()));
                    Ok(())
                }
                Err(refusal) => {
                    if refusal.outcome == Outcome::Error
                        && let (Some(device_id), Some(installation_id)) =
                            (&attempt.event.device_id, &attempt.event.installation_id)
                    {
                        lock(&shared.renewals).release(device_id, installation_id);
                    }
                    shared.finish(attempt, Err(&refusal));
                    Err(refusal.error)
                }
            }
        })
    }
}
