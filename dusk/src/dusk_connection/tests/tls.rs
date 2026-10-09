use capnp::capability::Promise;
use capnp_rpc::{RpcSystem, rpc_twoparty_capnp, twoparty};
use dusk_capnp::dusk_capnp::dusk;
use dusk_connection::{Connection, TlsClient};
use futures::io::AsyncReadExt;
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::task::JoinHandle;

const SERVER_NAME: &str = "node.fleet.test";

static DIRECTORIES: AtomicUsize = AtomicUsize::new(0);

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dusk-connection-tls-{}-{}",
            std::process::id(),
            DIRECTORIES.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Directory(path)
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("couldn't remove {}: {error}", self.0.display());
        }
    }
}

struct Authority {
    issuer: CertifiedIssuer<'static, KeyPair>,
}

struct Leaf {
    certificate: CertificateDer<'static>,
    certificate_pem: String,
    key_pem: String,
    key: PrivateKeyDer<'static>,
}

impl Authority {
    fn new(name: &str) -> Self {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        Authority {
            issuer: CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap(),
        }
    }

    fn leaf(&self, names: &[&str]) -> Leaf {
        let key = KeyPair::generate().unwrap();
        let params = CertificateParams::new(
            names
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let certificate = params.signed_by(&key, &self.issuer).unwrap();
        Leaf {
            certificate: certificate.der().clone(),
            certificate_pem: certificate.pem(),
            key_pem: key.serialize_pem(),
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        }
    }

    fn certificate(&self) -> CertificateDer<'static> {
        self.issuer.der().clone()
    }
}

struct Node {
    name: String,
}

impl dusk::Server for Node {
    fn hostname(
        &mut self,
        _params: dusk::HostnameParams,
        mut results: dusk::HostnameResults,
    ) -> Promise<(), capnp::Error> {
        results.get().set_result(self.name.as_str());
        Promise::ok(())
    }
}

#[derive(Default)]
struct Accepted {
    peers: RefCell<Vec<Option<CertificateDer<'static>>>>,
    connections: RefCell<Vec<JoinHandle<()>>>,
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

async fn serve(authority: &Authority, server: Leaf) -> (u16, Rc<Accepted>) {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(authority.certificate()).unwrap();
    let verifier =
        rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider())
            .build()
            .unwrap();
    let config = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![server.certificate], server.key)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Rc::new(Accepted::default());
    let recorder = accepted.clone();
    tokio::task::spawn_local(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let stream = match acceptor.accept(stream).await {
                Ok(stream) => stream,
                Err(error) => {
                    eprintln!("the test server refused a handshake: {error}");
                    continue;
                }
            };
            let peer = stream
                .get_ref()
                .1
                .peer_certificates()
                .and_then(|chain| chain.first().cloned());
            recorder.peers.borrow_mut().push(peer);
            let name = format!("connection-{}", recorder.peers.borrow().len());
            let (reader, writer) =
                tokio_util::compat::TokioAsyncReadCompatExt::compat(stream).split();
            let network = Box::new(twoparty::VatNetwork::new(
                reader,
                writer,
                rpc_twoparty_capnp::Side::Server,
                Default::default(),
            ));
            let bootstrap: dusk::Client = capnp_rpc::new_client(Node { name });
            let system = RpcSystem::new(network, Some(bootstrap.client));
            recorder
                .connections
                .borrow_mut()
                .push(tokio::task::spawn_local(async move {
                    if let Err(error) = system.await {
                        eprintln!("a test connection ended with {error}");
                    }
                }));
        }
    });
    (port, accepted)
}

async fn hostname(connection: &Connection) -> Result<String, capnp::Error> {
    let reply = connection
        .client()
        .await
        .hostname_request()
        .send()
        .promise
        .await?;
    Ok(reply.get()?.get_result()?.to_string()?)
}

struct Fixture {
    directory: Directory,
    authority: Authority,
    ca: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = Directory::new();
        let authority = Authority::new("fleet test authority");
        let ca = directory.write("ca.pem", &authority.issuer.pem());
        Fixture {
            directory,
            authority,
            ca,
        }
    }

    fn client(&self, leaf: &Leaf) -> TlsClient {
        TlsClient {
            server_name: SERVER_NAME.to_string(),
            ca: self.ca.clone(),
            certificate: Some(self.directory.write("client.pem", &leaf.certificate_pem)),
            key: Some(self.directory.write("client.key", &leaf.key_pem)),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn test_a_tls_connection_reaches_the_node_by_host_name_or_address_with_its_certificate() {
    let fixture = Fixture::new();
    let client = fixture.authority.leaf(&["client.test"]);
    let tls = fixture.client(&client);
    tokio::task::LocalSet::new()
        .run_until(async move {
            let (port, accepted) =
                serve(&fixture.authority, fixture.authority.leaf(&[SERVER_NAME])).await;
            for (index, host) in ["localhost", "127.0.0.1"].into_iter().enumerate() {
                let connection = Connection::connect_tls(host, port, tls.clone())
                    .await
                    .unwrap();
                assert_eq!(
                    hostname(&connection).await.unwrap(),
                    format!("connection-{}", index + 1)
                );
                assert_eq!(
                    accepted.peers.borrow()[index].as_ref(),
                    Some(&client.certificate),
                    "the server saw another client certificate"
                );
                connection.disconnect().await.unwrap();
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_a_server_name_the_server_certificate_does_not_carry_is_refused() {
    let fixture = Fixture::new();
    let tls = fixture.client(&fixture.authority.leaf(&["client.test"]));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let (port, accepted) =
                serve(&fixture.authority, fixture.authority.leaf(&[SERVER_NAME])).await;
            let tls = TlsClient {
                server_name: "other.fleet.test".to_string(),
                ..tls
            };
            let connection = Connection::connect_tls("127.0.0.1", port, tls)
                .await
                .unwrap();
            let error = hostname(&connection).await.unwrap_err();
            assert!(
                error.to_string().contains("TLS handshake"),
                "unexpected error: {error}"
            );
            assert!(accepted.peers.borrow().is_empty());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_a_server_that_requires_a_client_certificate_refuses_a_client_without_one() {
    let fixture = Fixture::new();
    let tls = TlsClient {
        server_name: SERVER_NAME.to_string(),
        ca: fixture.ca.clone(),
        certificate: None,
        key: None,
    };
    tokio::task::LocalSet::new()
        .run_until(async move {
            let (port, accepted) =
                serve(&fixture.authority, fixture.authority.leaf(&[SERVER_NAME])).await;
            let connection = Connection::connect_tls("127.0.0.1", port, tls)
                .await
                .unwrap();
            for _ in 0..2 {
                let error = hostname(&connection).await.unwrap_err();
                assert_eq!(error.kind, capnp::ErrorKind::Disconnected, "{error}");
            }
            assert!(accepted.peers.borrow().is_empty());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_a_refused_client_certificate_is_tried_again_once_a_valid_one_is_on_disk() {
    let fixture = Fixture::new();
    let valid = fixture.authority.leaf(&["client.test"]);
    let tls = fixture.client(&Authority::new("another authority").leaf(&["client.test"]));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let (port, accepted) =
                serve(&fixture.authority, fixture.authority.leaf(&[SERVER_NAME])).await;
            let connection = Connection::connect_tls("127.0.0.1", port, tls)
                .await
                .unwrap();
            let error = hostname(&connection).await.unwrap_err();
            assert_eq!(error.kind, capnp::ErrorKind::Disconnected, "{error}");
            assert!(accepted.peers.borrow().is_empty());

            fixture.client(&valid);
            let mut answer = hostname(&connection).await;
            for _ in 0..3 {
                if answer.is_ok() {
                    break;
                }
                answer = hostname(&connection).await;
            }
            assert_eq!(answer.unwrap(), "connection-1");
            assert_eq!(
                accepted.peers.borrow()[0].as_ref(),
                Some(&valid.certificate)
            );
            connection.disconnect().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_a_server_that_never_answers_the_handshake_gives_up_after_the_handshake_timeout() {
    let fixture = Fixture::new();
    let tls = fixture.client(&fixture.authority.leaf(&["client.test"]));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let silent = tokio::task::spawn_local(async move {
                let mut held = Vec::new();
                loop {
                    let (stream, _) = listener.accept().await.unwrap();
                    held.push(stream);
                }
            });
            let connection = Connection::connect_tls("127.0.0.1", port, tls)
                .await
                .unwrap();
            let started = std::time::Instant::now();
            let error = hostname(&connection).await.unwrap_err();
            let waited = started.elapsed();
            assert_eq!(error.kind, capnp::ErrorKind::Disconnected, "{error}");
            assert!(error.to_string().contains("TLS handshake"), "{error}");
            assert!(
                waited >= dusk_connection::HANDSHAKE_TIMEOUT
                    && waited < dusk_connection::HANDSHAKE_TIMEOUT * 2,
                "gave up after {waited:?}"
            );
            silent.abort();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_a_server_signed_by_another_authority_is_refused() {
    let fixture = Fixture::new();
    let tls = fixture.client(&fixture.authority.leaf(&["client.test"]));
    let stranger = Authority::new("another authority");
    tokio::task::LocalSet::new()
        .run_until(async move {
            let (port, _) = serve(&fixture.authority, stranger.leaf(&[SERVER_NAME])).await;
            let connection = Connection::connect_tls("127.0.0.1", port, tls)
                .await
                .unwrap();
            let error = hostname(&connection).await.unwrap_err();
            assert!(
                error.to_string().contains("TLS handshake"),
                "unexpected error: {error}"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_a_reconnect_reads_the_certificate_files_again() {
    let fixture = Fixture::new();
    let first = fixture.authority.leaf(&["first.client.test"]);
    let second = fixture.authority.leaf(&["second.client.test"]);
    let tls = fixture.client(&first);
    tokio::task::LocalSet::new()
        .run_until(async move {
            let (port, accepted) =
                serve(&fixture.authority, fixture.authority.leaf(&[SERVER_NAME])).await;
            let connection = Connection::connect_tls("127.0.0.1", port, tls)
                .await
                .unwrap();
            assert_eq!(hostname(&connection).await.unwrap(), "connection-1");

            fixture.client(&second);
            accepted.connections.borrow()[0].abort();
            let mut answer = hostname(&connection).await;
            for _ in 0..3 {
                if answer.is_ok() {
                    break;
                }
                answer = hostname(&connection).await;
            }
            assert_eq!(answer.unwrap(), "connection-2");
            assert_eq!(
                accepted.peers.borrow()[1].as_ref(),
                Some(&second.certificate)
            );
            connection.disconnect().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_unreadable_trust_anchors_fail_the_connect() {
    let directory = Directory::new();
    let tls = TlsClient {
        server_name: SERVER_NAME.to_string(),
        ca: directory.0.join("missing.pem"),
        certificate: None,
        key: None,
    };
    let Err(error) = Connection::connect_tls("127.0.0.1", 1, tls).await else {
        panic!("a missing trust anchor file was accepted");
    };
    assert!(format!("{error:#}").contains("missing.pem"), "{error:#}");

    let empty = TlsClient {
        server_name: SERVER_NAME.to_string(),
        ca: directory.write("empty.pem", ""),
        certificate: None,
        key: None,
    };
    assert!(
        Connection::connect_tls("127.0.0.1", 1, empty)
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_a_refused_handshake_is_tried_again_on_the_next_call() {
    let fixture = Fixture::new();
    let tls = fixture.client(&fixture.authority.leaf(&["client.test"]));
    let trusted = std::fs::read_to_string(&fixture.ca).unwrap();
    std::fs::write(
        &fixture.ca,
        Authority::new("another authority").issuer.pem(),
    )
    .unwrap();
    tokio::task::LocalSet::new()
        .run_until(async move {
            let (port, accepted) =
                serve(&fixture.authority, fixture.authority.leaf(&[SERVER_NAME])).await;
            let connection = Connection::connect_tls("127.0.0.1", port, tls)
                .await
                .unwrap();
            let error = hostname(&connection).await.unwrap_err();
            assert_eq!(error.kind, capnp::ErrorKind::Disconnected, "{error}");
            assert!(accepted.peers.borrow().is_empty());

            std::fs::write(&fixture.ca, trusted).unwrap();
            let mut answer = hostname(&connection).await;
            for _ in 0..3 {
                if answer.is_ok() {
                    break;
                }
                answer = hostname(&connection).await;
            }
            assert_eq!(answer.unwrap(), "connection-1");
            connection.disconnect().await.unwrap();
        })
        .await;
}
