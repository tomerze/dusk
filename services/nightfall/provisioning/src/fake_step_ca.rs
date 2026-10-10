use std::collections::{BTreeSet, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rcgen::{CertificateParams, KeyPair, SanType};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use x509_parser::extensions::{GeneralName, ParsedExtension};
use x509_parser::prelude::FromDer;

use crate::jwt::Jwks;
use crate::renew::fixtures::Authority;
use crate::step_ca::pem_encode;

#[derive(Default)]
pub(crate) struct Behaviour {
    pub(crate) delay: Option<Duration>,
    pub(crate) status: Option<u16>,
    pub(crate) oversized: bool,
}

pub(crate) struct FakeStepCa {
    pub(crate) url: String,
    pub(crate) tls_root: CertificateDer<'static>,
    pub(crate) authority: Arc<Authority>,
    pub(crate) behaviour: Arc<Mutex<Behaviour>>,
    pub(crate) tokens: Arc<Mutex<Vec<Value>>>,
}

struct Shared {
    authority: Arc<Authority>,
    provisioner: Jwks,
    audience: String,
    used: Mutex<HashSet<String>>,
    behaviour: Arc<Mutex<Behaviour>>,
    tokens: Arc<Mutex<Vec<Value>>>,
}

impl FakeStepCa {
    pub(crate) async fn start(provisioner_public_jwk: Value) -> FakeStepCa {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address: SocketAddr = listener.local_addr().unwrap();
        let url = format!("https://{address}");
        let tls_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut tls_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        tls_params.subject_alt_names = vec![SanType::IpAddress(address.ip())];
        let tls_certificate = tls_params.self_signed(&tls_key).unwrap();
        let tls_root = tls_certificate.der().clone();
        let server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![tls_root.clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(tls_key.serialize_der())),
        )
        .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(server_config));
        let authority = Arc::new(Authority::new("fleet-client"));
        let behaviour = Arc::new(Mutex::new(Behaviour::default()));
        let tokens = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::new(Shared {
            authority: authority.clone(),
            provisioner: Jwks::from_json(&json!({ "keys": [provisioner_public_jwk] }).to_string())
                .unwrap(),
            audience: format!("{url}/1.0/sign"),
            used: Mutex::new(HashSet::new()),
            behaviour: behaviour.clone(),
            tokens: tokens.clone(),
        });
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let acceptor = acceptor.clone();
                let shared = shared.clone();
                tokio::spawn(async move {
                    let Ok(mut stream) = acceptor.accept(stream).await else {
                        return;
                    };
                    while let Some(request) = read_request(&mut stream).await {
                        let delay = shared.behaviour.lock().unwrap().delay;
                        if let Some(delay) = delay {
                            tokio::time::sleep(delay).await;
                        }
                        let (status, body) = answer(&shared, &request);
                        let response = format!(
                            "HTTP/1.1 {status} Status\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                            body.len()
                        );
                        if stream.write_all(response.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        FakeStepCa {
            url,
            tls_root,
            authority,
            behaviour,
            tokens,
        }
    }
}

async fn read_request(stream: &mut (impl AsyncReadExt + Unpin)) -> Option<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]).to_lowercase();
    let length: usize = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    while buffer.len() < header_end + length {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    Some(buffer[header_end..header_end + length].to_vec())
}

fn refuse(status: u16, message: &str) -> (u16, String) {
    (
        status,
        json!({ "status": status, "message": message }).to_string(),
    )
}

fn answer(shared: &Shared, body: &[u8]) -> (u16, String) {
    if let Some(status) = shared.behaviour.lock().unwrap().status {
        return refuse(status, "configured failure");
    }
    if shared.behaviour.lock().unwrap().oversized {
        return (201, " ".repeat(crate::step_ca::MAXIMUM_RESPONSE_BYTES + 1));
    }
    let Ok(request) = serde_json::from_slice::<Value>(body) else {
        return refuse(400, "not JSON");
    };
    let Some(ott) = request["ott"].as_str() else {
        return refuse(400, "no ott");
    };
    let Ok(claims) = shared.provisioner.verify(ott) else {
        return refuse(401, "invalid token");
    };
    shared
        .tokens
        .lock()
        .unwrap()
        .push(Value::Object(claims.clone()));
    if claims["aud"] != shared.audience.as_str() || claims["iss"] != "nightfall" {
        return refuse(401, "wrong audience or issuer");
    }
    let Some(token_id) = claims["jti"].as_str() else {
        return refuse(401, "no jti");
    };
    if !shared.used.lock().unwrap().insert(String::from(token_id)) {
        return refuse(401, "token already used");
    }
    let Some(csr_pem) = request["csr"].as_str() else {
        return refuse(400, "no csr");
    };
    let Some(csr) = decode_pem(csr_pem, "CERTIFICATE REQUEST") else {
        return refuse(400, "csr is not PEM");
    };
    let Ok((_, parsed)) =
        x509_parser::certification_request::X509CertificationRequest::from_der(&csr)
    else {
        return refuse(400, "csr does not parse");
    };
    let mut requested = BTreeSet::new();
    for extension in parsed.requested_extensions().into_iter().flatten() {
        if let ParsedExtension::SubjectAlternativeName(names) = extension {
            for name in &names.general_names {
                if let GeneralName::URI(uri) = name {
                    requested.insert(String::from(*uri));
                }
            }
        }
    }
    let token_sans: BTreeSet<String> = claims["sans"]
        .as_array()
        .map(|sans| {
            sans.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    if requested != token_sans {
        return refuse(403, "csr sans do not match the token");
    }
    let lifetime = request["notAfter"]
        .as_str()
        .and_then(|value| value.strip_suffix('s'))
        .and_then(|seconds| seconds.parse::<i64>().ok())
        .unwrap_or(24 * 3600);
    let mut extra: Vec<String> = claims
        .get("tenant")
        .and_then(Value::as_str)
        .map(|tenant| vec![format!("urn:dusk:tenant:{tenant}")])
        .unwrap_or_default();
    if claims.get("attestation").and_then(Value::as_str) == Some("tpm") {
        extra.push(String::from(crate::identity::TPM_ATTESTATION_URI));
    }
    let leaf = shared
        .authority
        .sign_request(&csr, &extra, time::Duration::seconds(lifetime));
    let leaf_pem = pem_encode("CERTIFICATE", &leaf);
    let intermediate_pem = pem_encode("CERTIFICATE", &shared.authority.intermediate);
    (201, json!({ "crt": leaf_pem, "ca": intermediate_pem, "certChain": [leaf_pem, intermediate_pem] }).to_string())
}

fn decode_pem(pem: &str, label: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let body = pem.split_once(&begin)?.1.split_once(&end)?.0;
    let compact: String = body
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(compact)
        .ok()
}
