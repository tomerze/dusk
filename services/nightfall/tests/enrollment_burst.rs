use capnp::capability::Promise;
use capnp::message::ReaderOptions;
use capnp_rpc::rpc_twoparty_capnp::Side;
use capnp_rpc::{RpcSystem, twoparty};
use dusk_capnp::dusk_capnp::dusk;
use nightfall_provisioning::provision_capnp::provisioning;
use ring::rand::{SecureRandom, SystemRandom};
use rustls::ClientConfig;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

struct Settings {
    address: String,
    provision_name: String,
    fleet_name: String,
    roots: Vec<CertificateDer<'static>>,
    fleet_token: String,
    credential: String,
    cap: usize,
    attempts: usize,
    workers: usize,
    twilight: String,
    twilight_token: String,
}

fn variable(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
}

fn file(name: &str) -> String {
    let path = variable(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {path}: {error}"))
        .trim()
        .to_string()
}

fn settings() -> Settings {
    let number = |name: &str| {
        variable(name)
            .parse::<usize>()
            .unwrap_or_else(|_| panic!("{name} is not a whole number"))
    };
    let roots = CertificateDer::pem_file_iter(variable("NIGHTFALL_BURST_CA"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    Settings {
        address: variable("NIGHTFALL_BURST_ADDRESS"),
        provision_name: variable("NIGHTFALL_BURST_PROVISION_NAME"),
        fleet_name: variable("NIGHTFALL_BURST_FLEET_NAME"),
        roots,
        fleet_token: file("NIGHTFALL_BURST_FLEET_TOKEN_FILE"),
        credential: variable("NIGHTFALL_BURST_CREDENTIAL"),
        cap: number("NIGHTFALL_BURST_CAP"),
        attempts: number("NIGHTFALL_BURST_ATTEMPTS"),
        workers: number("NIGHTFALL_BURST_WORKERS"),
        twilight: variable("TWILIGHT_BURST_ADDRESS"),
        twilight_token: file("TWILIGHT_BURST_TOKEN_FILE"),
    }
}

fn client_config(
    settings: &Settings,
    identity: Option<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)>,
) -> Arc<ClientConfig> {
    let mut store = rustls::RootCertStore::empty();
    for root in &settings.roots {
        store.add(root.clone()).unwrap();
    }
    let builder =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_root_certificates(store);
    Arc::new(match identity {
        Some((chain, key)) => builder.with_client_auth_cert(chain, key).unwrap(),
        None => builder.with_no_client_auth(),
    })
}

async fn provisioning_client(settings: &Settings) -> provisioning::Client {
    let tcp = TcpStream::connect(&settings.address).await.unwrap();
    let tls = tokio_rustls::TlsConnector::from(client_config(settings, None))
        .connect(
            ServerName::try_from(settings.provision_name.clone()).unwrap(),
            tcp,
        )
        .await
        .unwrap();
    let (reader, writer) = tokio::io::split(tls);
    let network = twoparty::VatNetwork::new(
        reader.compat(),
        writer.compat_write(),
        Side::Client,
        ReaderOptions::new(),
    );
    let mut system = RpcSystem::new(Box::new(network), None);
    let client: provisioning::Client = system.bootstrap(Side::Server);
    tokio::task::spawn_local(async move {
        if let Err(error) = system.await {
            eprintln!("a provisioning link ended: {error}");
        }
    });
    client
}

fn device_report(
    mut device: nightfall_provisioning::provision_capnp::device_report::Builder<'_>,
    fingerprint: &[u8; 32],
) {
    device.set_hardware_fingerprint(fingerprint);
    device.set_installation_hint("");
    device.set_dusk_version("0.1.0");
    device.set_impl("nix");
    device.set_target_os("linux");
    device.set_target_arch("x86_64");
    device.set_hostname("burst");
}

struct Enrolled {
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

async fn enroll(
    client: &provisioning::Client,
    token: &str,
    fingerprint: &[u8; 32],
) -> Result<Enrolled, capnp::Error> {
    let mut assign = client.assign_request();
    assign.get().init_credential().set_fleet_token(token);
    device_report(assign.get().init_device(), fingerprint);
    let assigned = assign.send().promise.await?;
    let assignment = assigned.get()?.get_assignment()?;
    let device_id = assignment.get_device_id()?.to_string().unwrap();
    let installation_id = assignment.get_installation_id()?.to_string().unwrap();
    let challenge = assignment.get_challenge()?.to_vec();
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut parameters = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    parameters.distinguished_name = rcgen::DistinguishedName::new();
    parameters.subject_alt_names = vec![
        rcgen::SanType::URI(format!("urn:dusk:device:{device_id}").try_into().unwrap()),
        rcgen::SanType::URI(
            format!("urn:dusk:installation:{installation_id}")
                .try_into()
                .unwrap(),
        ),
    ];
    let csr = parameters.serialize_request(&key).unwrap();
    let mut request = client.enroll_request();
    request.get().init_credential().set_fleet_token(token);
    device_report(request.get().init_device(), fingerprint);
    request.get().set_challenge(&challenge);
    request.get().set_csr(csr.der());
    let issued = request.send().promise.await?;
    let chain = issued
        .get()?
        .get_issued()?
        .get_certificate_chain()?
        .iter()
        .map(|certificate| CertificateDer::from(certificate.unwrap().to_vec()))
        .collect();
    Ok(Enrolled {
        chain,
        key: PrivateKeyDer::Pkcs8(key.serialize_der().into()),
    })
}

struct FakeNode {
    namespace_id: u64,
}

impl dusk::Server for FakeNode {
    fn namespace_id(
        &mut self,
        _params: dusk::NamespaceIdParams,
        mut results: dusk::NamespaceIdResults,
    ) -> Promise<(), capnp::Error> {
        results.get().set_result(self.namespace_id);
        Promise::ok(())
    }

    fn programs(
        &mut self,
        _params: dusk::ProgramsParams,
        mut results: dusk::ProgramsResults,
    ) -> Promise<(), capnp::Error> {
        results.get().init_program_entries(0);
        Promise::ok(())
    }

    fn dusk(
        &mut self,
        _params: dusk::DuskParams,
        mut results: dusk::DuskResults,
    ) -> Promise<(), capnp::Error> {
        results.get().set_result(capnp_rpc::new_client(FakeNode {
            namespace_id: self.namespace_id,
        }));
        Promise::ok(())
    }

    fn time(
        &mut self,
        _params: dusk::TimeParams,
        mut results: dusk::TimeResults,
    ) -> Promise<(), capnp::Error> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        results.get().set_unix_time_ms(now);
        Promise::ok(())
    }
}

async fn link(settings: &Settings, enrolled: Enrolled, namespace_id: u64) {
    let tcp = TcpStream::connect(&settings.address).await.unwrap();
    let identity = client_config(settings, Some((enrolled.chain, enrolled.key)));
    let tls = tokio_rustls::TlsConnector::from(identity)
        .connect(
            ServerName::try_from(settings.fleet_name.clone()).unwrap(),
            tcp,
        )
        .await
        .unwrap();
    let (reader, writer) = tokio::io::split(tls);
    let network = twoparty::VatNetwork::new(
        reader.compat(),
        writer.compat_write(),
        Side::Server,
        ReaderOptions::new(),
    );
    let bootstrap: dusk::Client = capnp_rpc::new_client(FakeNode { namespace_id });
    let system = RpcSystem::new(Box::new(network), Some(bootstrap.client));
    let ended = system.await;
    eprintln!("a fake node's link ended: {ended:?}");
}

fn query_escape(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn dechunk(body: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::new();
    let mut rest = body;
    loop {
        let Some(end) = rest.windows(2).position(|pair| pair == b"\r\n") else {
            return decoded;
        };
        let size =
            usize::from_str_radix(std::str::from_utf8(&rest[..end]).unwrap().trim(), 16).unwrap();
        if size == 0 {
            return decoded;
        }
        decoded.extend_from_slice(&rest[end + 2..end + 2 + size]);
        rest = &rest[end + 2 + size + 2..];
    }
}

async fn twilight(
    settings: &Settings,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|body| body.to_string()).unwrap_or_default();
    let mut stream = TcpStream::connect(&settings.twilight).await.unwrap();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        settings.twilight,
        settings.twilight_token,
        payload.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("an HTTP response has a header");
    let head = String::from_utf8_lossy(&response[..split]).to_ascii_lowercase();
    let status: u16 = head.split(' ').nth(1).unwrap().parse().unwrap();
    let mut body = response[split + 4..].to_vec();
    if head.contains("transfer-encoding: chunked") {
        body = dechunk(&body);
    }
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

async fn enrolled_with(settings: &Settings) -> usize {
    let selector = format!(
        "credential_kind == \"fleet_token\" and credential_ref == \"{}\"",
        settings.credential
    );
    let (status, page) = twilight(
        settings,
        "GET",
        &format!(
            "/api/v1/nodes?limit=500&selector={}",
            query_escape(&selector)
        ),
        None,
    )
    .await;
    assert_eq!(status, 200, "{page}");
    page["items"].as_array().unwrap().len()
}

async fn open_alert(settings: &Settings, fingerprint: &str, timeout: Duration) -> Value {
    let deadline = Instant::now() + timeout;
    loop {
        let (status, page) =
            twilight(settings, "GET", "/api/v1/alerts?state=open&limit=500", None).await;
        assert_eq!(status, 200, "{page}");
        if let Some(alert) = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|alert| alert["fingerprint"] == fingerprint)
        {
            return alert.clone();
        }
        assert!(Instant::now() < deadline, "no open alert {fingerprint}");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[derive(Default)]
struct Tally {
    issued: Vec<Enrolled>,
    quota_refusals: usize,
    rate_limited: usize,
    other: Vec<String>,
}

#[test]
#[ignore = "enrolls fake installations against a running Dusk stack: set the NIGHTFALL_BURST_* and TWILIGHT_BURST_* variables"]
fn a_burst_with_one_fleet_token_meets_its_cap_raises_the_alerts_and_is_revoked_in_one_request() {
    let settings = Rc::new(settings());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    tokio::task::LocalSet::new().block_on(&runtime, async move {
        let before = enrolled_with(&settings).await;
        eprintln!("{before} installations were enrolled with {} before the burst", settings.credential);
        let tally = Rc::new(RefCell::new(Tally::default()));
        let next = Rc::new(RefCell::new(0usize));
        let started = Instant::now();
        let mut workers = Vec::new();
        for _ in 0..settings.workers {
            let settings = settings.clone();
            let tally = tally.clone();
            let next = next.clone();
            workers.push(tokio::task::spawn_local(async move {
                let client = provisioning_client(&settings).await;
                let random = SystemRandom::new();
                loop {
                    let attempt = {
                        let mut next = next.borrow_mut();
                        *next += 1;
                        *next
                    };
                    if attempt > settings.attempts {
                        return;
                    }
                    let mut fingerprint = [0u8; 32];
                    random.fill(&mut fingerprint).unwrap();
                    loop {
                        match enroll(&client, &settings.fleet_token, &fingerprint).await {
                            Ok(enrolled) => {
                                tally.borrow_mut().issued.push(enrolled);
                                break;
                            }
                            Err(error) if error.kind == capnp::ErrorKind::Overloaded => {
                                tally.borrow_mut().rate_limited += 1;
                                tokio::time::sleep(Duration::from_millis(200)).await;
                            }
                            Err(error) if error.extra.contains("credential quota reached") => {
                                tally.borrow_mut().quota_refusals += 1;
                                break;
                            }
                            Err(error) => {
                                tally.borrow_mut().other.push(error.to_string());
                                break;
                            }
                        }
                    }
                }
            }));
        }
        for worker in workers {
            worker.await.unwrap();
        }
        let elapsed = started.elapsed();
        let issued = std::mem::take(&mut tally.borrow_mut().issued);
        let (quota_refusals, rate_limited, other) = {
            let tally = tally.borrow();
            (tally.quota_refusals, tally.rate_limited, tally.other.clone())
        };
        eprintln!(
            "burst: {} attempts in {:.1} s, {} issued ({:.2} a second), {} refused for the cap, {} refused as overloaded and retried, other failures {:?}",
            settings.attempts,
            elapsed.as_secs_f64(),
            issued.len(),
            issued.len() as f64 / elapsed.as_secs_f64(),
            quota_refusals,
            rate_limited,
            other
        );
        assert!(other.is_empty(), "{other:?}");
        assert_eq!(before + issued.len(), settings.cap, "the cap was not met exactly");
        assert_eq!(issued.len() + quota_refusals, settings.attempts);

        let mut links = Vec::new();
        let random = SystemRandom::new();
        for enrolled in issued {
            let mut namespace = [0u8; 8];
            random.fill(&mut namespace).unwrap();
            let settings = settings.clone();
            links.push(tokio::task::spawn_local(async move {
                link(&settings, enrolled, u64::from_be_bytes(namespace)).await;
            }));
        }
        let fake_count = links.len();

        let fingerprint = format!("enrollment_rate:credential:fleet_token:{}", settings.credential);
        let rate = open_alert(&settings, &fingerprint, Duration::from_secs(120)).await;
        eprintln!("open alert: {rate}");
        assert_eq!(rate["detail"]["credential"], settings.credential.as_str());
        assert_eq!(rate["severity"], "high");
        let quota = open_alert(
            &settings,
            &format!("credential_quota_reached:credential:fleet_token:{}", settings.credential),
            Duration::from_secs(60),
        )
        .await;
        eprintln!("open alert: {quota}");
        assert_eq!(quota["detail"]["credential"], settings.credential.as_str());

        let request = json!({"credential_kind": "fleet_token", "credential": settings.credential, "reason": "burst test"});
        let mut counting = request.clone();
        counting["dry_run"] = Value::Bool(true);
        let (status, counted) = twilight(&settings, "POST", "/api/v1/revocations", Some(counting)).await;
        assert_eq!(status, 200, "{counted}");
        assert_eq!(counted["matched"], settings.cap);
        let revoking = Instant::now();
        let (status, started) = twilight(&settings, "POST", "/api/v1/revocations", Some(request)).await;
        assert_eq!(status, 202, "{started}");
        let identifier = started["revocation"]["id"].as_str().unwrap().to_string();
        let finished = loop {
            let (status, revocation) =
                twilight(&settings, "GET", &format!("/api/v1/revocations/{identifier}"), None).await;
            assert_eq!(status, 200, "{revocation}");
            if !revocation["finished_at"].is_null() {
                break revocation;
            }
            assert!(revoking.elapsed() < Duration::from_secs(120), "the revocation did not finish: {revocation}");
            tokio::time::sleep(Duration::from_millis(500)).await;
        };
        eprintln!("revocation finished in {:.1} s: {finished}", revoking.elapsed().as_secs_f64());
        assert_eq!(finished["revoked"], settings.cap);
        let deadline = Instant::now() + Duration::from_secs(60);
        while links.iter().any(|link| !link.is_finished()) {
            assert!(Instant::now() < deadline, "nightfall kept fake nodes linked after the revocation");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        eprintln!(
            "every one of the {fake_count} fake nodes was dropped {:.1} s after the revocation started",
            revoking.elapsed().as_secs_f64()
        );
        assert_eq!(enrolled_with(&settings).await, settings.cap);
    });
}
