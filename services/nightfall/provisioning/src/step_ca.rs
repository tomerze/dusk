use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tracing::debug;

use crate::config::{ConfigError, load_certificates};
use crate::credential::sha256;
use crate::jwt::SigningKey;

pub const ONE_TIME_TOKEN_SECONDS: u64 = 300;
pub const MAXIMUM_RESPONSE_BYTES: usize = 1 << 20;

pub struct StepCaConfig {
    pub url: String,
    pub roots: Vec<CertificateDer<'static>>,
    pub provisioner: String,
    pub provisioner_key: SigningKey,
    pub certificate_lifetime: Duration,
    pub max_concurrent: usize,
    pub timeout: Duration,
}

impl StepCaConfig {
    pub fn load(
        url: &str,
        root: &Path,
        provisioner: &str,
        provisioner_key_file: &Path,
        certificate_lifetime: Duration,
    ) -> Result<StepCaConfig, ConfigError> {
        let key_text = std::fs::read_to_string(provisioner_key_file)
            .map_err(|source| ConfigError::read(provisioner_key_file, source))?;
        let provisioner_key = SigningKey::from_jwk(&key_text)
            .map_err(|error| ConfigError::invalid(provisioner_key_file, error.to_string()))?;
        Ok(StepCaConfig {
            url: String::from(url),
            roots: load_certificates(root)?,
            provisioner: String::from(provisioner),
            provisioner_key,
            certificate_lifetime,
            max_concurrent: 16,
            timeout: Duration::from_millis(10_000),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StepCaError {
    #[error("step-ca already handles the configured number of concurrent requests")]
    Overloaded,
    #[error("step-ca did not answer within {0:?}")]
    TimedOut(Duration),
    #[error("step-ca refused the token ({status}): {message}")]
    Unauthorized { status: u16, message: String },
    #[error("step-ca refused the request ({status}): {message}")]
    Refused { status: u16, message: String },
    #[error("step-ca failed ({status}): {message}")]
    Failed { status: u16, message: String },
    #[error("reaching step-ca failed: {0}")]
    Transport(String),
    #[error("step-ca answered with something that is not a certificate chain: {0}")]
    InvalidResponse(String),
    #[error("minting the one-time token failed: {0}")]
    Token(String),
    #[error("{0}")]
    Configuration(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignRequest {
    pub csr_der: Vec<u8>,
    pub subject: String,
    pub sans: Vec<String>,
    pub tenant: Option<String>,
    pub token_id: String,
    pub tpm_bound: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedCertificate {
    pub chain: Vec<CertificateDer<'static>>,
    pub serial: String,
    pub fingerprint: String,
    pub not_before_unix: i64,
    pub not_after_unix: i64,
}

#[derive(Deserialize)]
struct SignResponse {
    crt: Option<String>,
    ca: Option<String>,
    #[serde(rename = "certChain", default)]
    certificate_chain: Vec<String>,
}

pub struct StepCaClient {
    http: reqwest::Client,
    sign_url: String,
    provisioner: String,
    key: SigningKey,
    lifetime: Duration,
    permits: Arc<Semaphore>,
    timeout: Duration,
}

pub fn pem_encode(label: &str, der: &[u8]) -> String {
    let encoded = STANDARD.encode(der);
    let mut pem = format!("-----BEGIN {label}-----\n");
    for line in encoded.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
        pem.push('\n');
    }
    pem.push_str(&format!("-----END {label}-----\n"));
    pem
}

pub fn go_duration(duration: Duration) -> String {
    format!("{}s", duration.as_secs())
}

pub fn describe_certificate(der: &[u8]) -> Result<(String, String, i64, i64), String> {
    let (_, certificate) = x509_parser::parse_x509_certificate(der)
        .map_err(|error| format!("not an X.509 certificate: {error}"))?;
    let serial_bytes = certificate.raw_serial();
    let significant = serial_bytes
        .iter()
        .position(|byte| *byte != 0)
        .map(|start| &serial_bytes[start..])
        .unwrap_or(&serial_bytes[serial_bytes.len().saturating_sub(1)..]);
    let serial = hex::encode(significant);
    let serial = serial.trim_start_matches('0');
    let serial = if serial.is_empty() {
        String::from("0")
    } else {
        String::from(serial)
    };
    let validity = certificate.validity();
    Ok((
        serial,
        hex::encode(sha256(der)),
        validity.not_before.timestamp(),
        validity.not_after.timestamp(),
    ))
}

impl StepCaClient {
    pub fn new(config: StepCaConfig) -> Result<StepCaClient, StepCaError> {
        let mut roots = rustls::RootCertStore::empty();
        for root in &config.roots {
            roots.add(root.clone()).map_err(|error| {
                StepCaError::Configuration(format!("the step-ca root is not usable: {error}"))
            })?;
        }
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|error| StepCaError::Configuration(error.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .no_proxy()
            .connect_timeout(config.timeout)
            .timeout(config.timeout)
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .map_err(|error| {
                StepCaError::Configuration(format!("building the step-ca client failed: {error}"))
            })?;
        if config.max_concurrent == 0 {
            return Err(StepCaError::Configuration(String::from(
                "max_concurrent must be at least 1",
            )));
        }
        Ok(StepCaClient {
            http,
            sign_url: format!("{}/1.0/sign", config.url.trim_end_matches('/')),
            provisioner: config.provisioner,
            key: config.provisioner_key,
            lifetime: config.certificate_lifetime,
            permits: Arc::new(Semaphore::new(config.max_concurrent)),
            timeout: config.timeout,
        })
    }

    pub fn certificate_lifetime(&self) -> Duration {
        self.lifetime
    }

    pub fn one_time_token(
        &self,
        request: &SignRequest,
        now_unix_seconds: u64,
    ) -> Result<String, StepCaError> {
        let mut claims = json!({
            "iss": self.provisioner,
            "aud": self.sign_url,
            "sub": request.subject,
            "sans": request.sans,
            "iat": now_unix_seconds,
            "nbf": now_unix_seconds,
            "exp": now_unix_seconds + ONE_TIME_TOKEN_SECONDS,
            "jti": request.token_id,
        });
        if let Some(tenant) = &request.tenant {
            claims["tenant"] = Value::from(tenant.as_str());
        }
        if request.tpm_bound {
            claims["attestation"] = Value::from("tpm");
        }
        self.key
            .sign(&claims)
            .map_err(|error| StepCaError::Token(error.to_string()))
    }

    pub async fn sign(
        &self,
        request: &SignRequest,
        now_unix_seconds: u64,
    ) -> Result<SignedCertificate, StepCaError> {
        let _permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| StepCaError::Overloaded)?;
        let body = json!({
            "csr": pem_encode("CERTIFICATE REQUEST", &request.csr_der),
            "ott": self.one_time_token(request, now_unix_seconds)?,
            "notAfter": go_duration(self.lifetime),
        });
        let exchange = async {
            let mut response = self.http.post(&self.sign_url).json(&body).send().await?;
            let status = response.status().as_u16();
            let mut received = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if received.len() + chunk.len() > MAXIMUM_RESPONSE_BYTES {
                    return Ok((status, None));
                }
                received.extend_from_slice(&chunk);
            }
            Ok::<(u16, Option<Vec<u8>>), reqwest::Error>((status, Some(received)))
        };
        let (status, received) = match tokio::time::timeout(self.timeout, exchange).await {
            Err(_) => return Err(StepCaError::TimedOut(self.timeout)),
            Ok(Err(error)) if error.is_timeout() => {
                return Err(StepCaError::TimedOut(self.timeout));
            }
            Ok(Err(error)) => return Err(StepCaError::Transport(format!("{error:#}"))),
            Ok(Ok(exchanged)) => exchanged,
        };
        let Some(received) = received else {
            return Err(StepCaError::InvalidResponse(format!(
                "the answer ({status}) is larger than {MAXIMUM_RESPONSE_BYTES} bytes"
            )));
        };
        let text = String::from_utf8_lossy(&received);
        debug!(status, url = %self.sign_url, "step-ca answered a sign request");
        if !(200..300).contains(&status) {
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|value| {
                    value
                        .get("message")
                        .and_then(Value::as_str)
                        .map(String::from)
                })
                .unwrap_or_else(|| text.chars().take(512).collect());
            return Err(match status {
                401 | 403 => StepCaError::Unauthorized { status, message },
                400..=499 => StepCaError::Refused { status, message },
                _ => StepCaError::Failed { status, message },
            });
        }
        let response: SignResponse = serde_json::from_str(&text)
            .map_err(|error| StepCaError::InvalidResponse(error.to_string()))?;
        let pems = if response.certificate_chain.is_empty() {
            response.crt.into_iter().chain(response.ca).collect()
        } else {
            response.certificate_chain
        };
        let mut chain = Vec::with_capacity(pems.len());
        for pem in &pems {
            for certificate in CertificateDer::pem_slice_iter(pem.as_bytes()) {
                chain.push(
                    certificate.map_err(|error| StepCaError::InvalidResponse(error.to_string()))?,
                );
            }
        }
        let leaf = chain
            .first()
            .ok_or_else(|| StepCaError::InvalidResponse(String::from("the chain is empty")))?;
        let (serial, fingerprint, not_before_unix, not_after_unix) =
            describe_certificate(leaf).map_err(StepCaError::InvalidResponse)?;
        Ok(SignedCertificate {
            chain,
            serial,
            fingerprint,
            not_before_unix,
            not_after_unix,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jwt::Jwks;
    use crate::jwt::fixtures::private_jwk;

    fn client() -> StepCaClient {
        let issuer = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let root = rcgen::CertificateParams::new(Vec::<String>::new())
            .unwrap()
            .self_signed(&issuer)
            .unwrap();
        StepCaClient::new(StepCaConfig {
            url: String::from("https://step-ca:9000/"),
            roots: vec![root.der().clone()],
            provisioner: String::from("nightfall"),
            provisioner_key: SigningKey::from_jwk(&private_jwk()).unwrap(),
            certificate_lifetime: Duration::from_secs(168 * 3600),
            max_concurrent: 1,
            timeout: Duration::from_secs(1),
        })
        .unwrap()
    }

    #[test]
    fn mints_the_one_time_token_step_ca_expects() {
        let client = client();
        let request = SignRequest {
            csr_der: Vec::new(),
            subject: String::from(
                "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13.a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70",
            ),
            sans: vec![
                String::from("urn:dusk:device:3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13"),
                String::from("urn:dusk:installation:a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70"),
            ],
            tenant: Some(String::from("retail-eu")),
            token_id: String::from("0011"),
            tpm_bound: false,
        };
        let token = client.one_time_token(&request, 1_791_278_043).unwrap();
        let jwks =
            Jwks::from_json(&json!({ "keys": [client.key.public_jwk()] }).to_string()).unwrap();
        let claims = jwks.verify(&token).unwrap();
        assert_eq!(claims["iss"], "nightfall");
        assert_eq!(claims["aud"], "https://step-ca:9000/1.0/sign");
        assert_eq!(claims["sub"], request.subject.as_str());
        assert_eq!(claims["sans"], json!(request.sans));
        assert_eq!(claims["nbf"], 1_791_278_043u64);
        assert_eq!(claims["exp"], 1_791_278_343u64);
        assert_eq!(claims["jti"], "0011");
        assert_eq!(claims["tenant"], "retail-eu");
        assert!(claims.get("attestation").is_none());
        let header: Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(token.split('.').next().unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(header["kid"], client.key.key_id());
        assert_eq!(header["alg"], "ES256");
        let attested = client
            .one_time_token(
                &SignRequest {
                    tpm_bound: true,
                    ..request.clone()
                },
                1_791_278_043,
            )
            .unwrap();
        assert_eq!(jwks.verify(&attested).unwrap()["attestation"], "tpm");
        let without_tenant = client
            .one_time_token(
                &SignRequest {
                    tenant: None,
                    ..request
                },
                1_791_278_043,
            )
            .unwrap();
        assert!(
            jwks.verify(&without_tenant)
                .unwrap()
                .get("tenant")
                .is_none()
        );
    }

    #[test]
    fn writes_pem_and_go_durations() {
        let pem = pem_encode("CERTIFICATE REQUEST", &[0u8; 100]);
        assert!(pem.starts_with("-----BEGIN CERTIFICATE REQUEST-----\n"));
        assert!(pem.lines().all(|line| line.len() <= 64));
        assert_eq!(go_duration(Duration::from_secs(168 * 3600)), "604800s");
    }
}
