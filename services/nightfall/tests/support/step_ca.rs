use super::pki::Authority;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use nightfall_provisioning::jwt::Jwks;
use rcgen::{CertificateParams, KeyPair, SanType};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub struct FakeStepCa {
    pub url: String,
    pub root_pem: String,
    pub signed: Arc<Mutex<Vec<Value>>>,
    thread: Option<std::thread::JoinHandle<()>>,
    stop: tokio_util::sync::CancellationToken,
}

fn pem(label: &str, der: &[u8]) -> String {
    let encoded = STANDARD.encode(der);
    let mut text = format!("-----BEGIN {label}-----\n");
    for line in encoded.as_bytes().chunks(64) {
        text.push_str(std::str::from_utf8(line).unwrap());
        text.push('\n');
    }
    text.push_str(&format!("-----END {label}-----\n"));
    text
}

fn from_pem(text: &str) -> Vec<u8> {
    let body: String = text
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    STANDARD.decode(body).unwrap()
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

struct Signer {
    authority: Arc<Authority>,
    provisioner: Jwks,
    used: Mutex<HashSet<String>>,
    signed: Arc<Mutex<Vec<Value>>>,
}

impl Signer {
    fn answer(&self, body: &[u8]) -> (u16, String) {
        let refuse = |status: u16, message: &str| {
            (
                status,
                json!({ "status": status, "message": message }).to_string(),
            )
        };
        let Ok(request) = serde_json::from_slice::<Value>(body) else {
            return refuse(400, "not JSON");
        };
        let Some(Ok(claims)) = request["ott"]
            .as_str()
            .map(|token| self.provisioner.verify(token))
        else {
            return refuse(401, "invalid token");
        };
        let Some(token_id) = claims["jti"].as_str() else {
            return refuse(401, "no jti");
        };
        if !self.used.lock().unwrap().insert(token_id.to_string()) {
            return refuse(401, "token already used");
        }
        let Some(csr) = request["csr"].as_str().map(from_pem) else {
            return refuse(400, "no csr");
        };
        let tenant: Vec<String> = claims
            .get("tenant")
            .and_then(Value::as_str)
            .map(|tenant| vec![format!("urn:dusk:tenant:{tenant}")])
            .unwrap_or_default();
        let lifetime = request["notAfter"]
            .as_str()
            .and_then(|value| nightfall::config::parse_duration(value).ok())
            .unwrap_or(Duration::from_secs(3600));
        let leaf = self.authority.sign_request(&csr, &tenant, lifetime);
        let leaf_pem = pem("CERTIFICATE", &leaf);
        let root_pem = self.authority.pem.clone();
        self.signed.lock().unwrap().push(Value::Object(claims));
        (
            201,
            json!({ "crt": leaf_pem, "ca": root_pem, "certChain": [leaf_pem, root_pem] })
                .to_string(),
        )
    }
}

impl FakeStepCa {
    pub fn start(provisioner_public_jwk: Value, authority: Arc<Authority>) -> FakeStepCa {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let tls_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut parameters = CertificateParams::new(Vec::<String>::new()).unwrap();
        parameters.subject_alt_names = vec![SanType::IpAddress(address.ip())];
        let certificate = parameters.self_signed(&tls_key).unwrap();
        let root: CertificateDer<'static> = certificate.der().clone();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![root.clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(tls_key.serialize_der())),
        )
        .unwrap();
        let signed = Arc::new(Mutex::new(Vec::new()));
        let signer = Arc::new(Signer {
            authority,
            provisioner: Jwks::from_json(&json!({ "keys": [provisioner_public_jwk] }).to_string())
                .unwrap(),
            used: Mutex::new(HashSet::new()),
            signed: signed.clone(),
        });
        let stop = tokio_util::sync::CancellationToken::new();
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
                loop {
                    let (stream, _) = tokio::select! {
                        () = stopping.cancelled() => return,
                        accepted = listener.accept() => accepted.unwrap(),
                    };
                    let acceptor = acceptor.clone();
                    let signer = signer.clone();
                    tokio::spawn(async move {
                        let Ok(mut stream) = acceptor.accept(stream).await else {
                            return;
                        };
                        while let Some(body) = read_request(&mut stream).await {
                            let (status, answer) = signer.answer(&body);
                            let response = format!(
                                "HTTP/1.1 {status} Status\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{answer}",
                                answer.len()
                            );
                            if stream.write_all(response.as_bytes()).await.is_err() {
                                return;
                            }
                        }
                    });
                }
            });
        });
        FakeStepCa {
            url: format!("https://{address}"),
            root_pem: pem("CERTIFICATE", &root),
            signed,
            thread: Some(thread),
            stop,
        }
    }
}

impl Drop for FakeStepCa {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            eprintln!("the fake step-ca thread panicked");
        }
    }
}
