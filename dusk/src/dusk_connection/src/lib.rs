use anyhow::Result;
use capnp_rpc::{Disconnector, RpcSystem, rpc_twoparty_capnp, twoparty};
use dusk_capnp::dusk_capnp::dusk::Client;
use futures::io::AsyncReadExt;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use std::{net::SocketAddr, rc::Rc, sync::Mutex};
use tokio::net::TcpStream;

type DisconnectorStore = Rc<Mutex<Option<Disconnector<rpc_twoparty_capnp::Side>>>>;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct TlsClient {
    pub server_name: String,
    pub ca: PathBuf,
    pub certificate: Option<PathBuf>,
    pub key: Option<PathBuf>,
}

impl TlsClient {
    fn config(&self) -> anyhow::Result<rustls::ClientConfig> {
        let mut roots = rustls::RootCertStore::empty();
        for certificate in read_certificates(&self.ca)? {
            roots.add(certificate).map_err(|error| {
                anyhow::anyhow!(
                    "`{}` holds a trust anchor rustls refuses: {error}",
                    self.ca.display()
                )
            })?;
        }
        let builder = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots);
        Ok(match (&self.certificate, &self.key) {
            (Some(certificate), Some(key)) => {
                let chain = read_certificates(certificate)?;
                let key = PrivateKeyDer::from_pem_file(key).map_err(|error| {
                    anyhow::anyhow!(
                        "couldn't read a private key from `{}`: {error}",
                        key.display()
                    )
                })?;
                builder.with_client_auth_cert(chain, key)?
            }
            (None, None) => builder.with_no_client_auth(),
            (Some(_), None) => anyhow::bail!("a client certificate needs its key"),
            (None, Some(_)) => anyhow::bail!("a client key needs its certificate"),
        })
    }
}

fn read_certificates(path: &Path) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let certificates = CertificateDer::pem_file_iter(path)
        .and_then(|certificates| certificates.collect::<Result<Vec<_>, _>>())
        .map_err(|error| {
            anyhow::anyhow!(
                "couldn't read certificates from `{}`: {error}",
                path.display()
            )
        })?;
    if certificates.is_empty() {
        anyhow::bail!("`{}` holds no PEM certificate", path.display());
    }
    Ok(certificates)
}

#[derive(Clone)]
enum Target {
    Plain(SocketAddr),
    Tls {
        host: String,
        port: u16,
        server_name: ServerName<'static>,
        tls: TlsClient,
    },
}

async fn dial(host: &str, port: u16) -> anyhow::Result<TcpStream> {
    let addresses = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "couldn't resolve `{host}`: no answer within {} seconds",
                CONNECT_TIMEOUT.as_secs()
            )
        })?
        .map_err(|error| anyhow::anyhow!("couldn't resolve `{host}`: {error}"))?;
    let mut failures = Vec::new();
    for address in addresses {
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address)).await {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(error)) => failures.push(format!("{address}: {error}")),
            Err(_) => failures.push(format!(
                "{address}: no answer within {} seconds",
                CONNECT_TIMEOUT.as_secs()
            )),
        }
    }
    if failures.is_empty() {
        anyhow::bail!("`{host}` resolves to no address");
    }
    anyhow::bail!(
        "couldn't connect to `{host}:{port}`: {}",
        failures.join("; ")
    )
}

struct LinkStream<Stream>(Stream);

fn link_failure(error: std::io::Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::ConnectionAborted, error.to_string())
}

impl<Stream: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for LinkStream<Stream> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match std::pin::Pin::new(&mut self.0).poll_read(context, buffer) {
            std::task::Poll::Ready(Err(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                std::task::Poll::Ready(Ok(()))
            }
            polled => polled.map_err(link_failure),
        }
    }
}

impl<Stream: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for LinkStream<Stream> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.0)
            .poll_write(context, bytes)
            .map_err(link_failure)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0)
            .poll_flush(context)
            .map_err(link_failure)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0)
            .poll_shutdown(context)
            .map_err(link_failure)
    }
}

fn serve<Stream>(stream: Stream, disconnector_store: &DisconnectorStore) -> capnp::Result<Client>
where
    Stream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + 'static,
{
    let stream = tokio_util::compat::TokioAsyncReadCompatExt::compat(stream);
    let (reader, writer) = stream.split();
    let rpc_network = Box::new(twoparty::VatNetwork::new(
        reader,
        writer,
        rpc_twoparty_capnp::Side::Client,
        Default::default(),
    ));
    let mut rpc_system = RpcSystem::new(rpc_network, None);
    let disconnector = rpc_system.get_disconnector();
    {
        let mut disconnector_option_guard = disconnector_store
            .lock()
            .map_err(|error| capnp::Error::failed(error.to_string()))?;
        *disconnector_option_guard = Some(disconnector);
    }
    let client: Client = rpc_system.bootstrap(rpc_twoparty_capnp::Side::Server);
    tokio::task::spawn_local(rpc_system);
    Ok(client)
}

async fn open(target: Target, disconnector_store: DisconnectorStore) -> capnp::Result<Client> {
    match target {
        Target::Plain(address) => {
            let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address))
                .await
                .map_err(|_| {
                    capnp::Error::disconnected(format!(
                        "couldn't connect to {address}: no answer within {} seconds",
                        CONNECT_TIMEOUT.as_secs()
                    ))
                })?
                .map_err(|error| capnp::Error::disconnected(error.to_string()))?;
            stream
                .set_nodelay(true)
                .map_err(|error| capnp::Error::disconnected(error.to_string()))?;
            serve(LinkStream(stream), &disconnector_store)
        }
        Target::Tls {
            host,
            port,
            server_name,
            tls,
        } => {
            let unreachable =
                |error: anyhow::Error| capnp::Error::disconnected(format!("{error:#}"));
            let config = tls.config().map_err(unreachable)?;
            let stream = dial(&host, port).await.map_err(unreachable)?;
            stream
                .set_nodelay(true)
                .map_err(|error| capnp::Error::disconnected(error.to_string()))?;
            let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
            let stream = tokio::time::timeout(
                HANDSHAKE_TIMEOUT,
                connector.connect(server_name.clone(), stream),
            )
            .await
            .map_err(|_| {
                capnp::Error::disconnected(format!(
                    "the TLS handshake with `{host}:{port}` took longer than {} seconds",
                    HANDSHAKE_TIMEOUT.as_secs()
                ))
            })?
            .map_err(|error| {
                capnp::Error::disconnected(format!(
                    "the TLS handshake with `{host}:{port}` as `{}` failed: {error}",
                    server_name.to_str()
                ))
            })?;
            serve(LinkStream(stream), &disconnector_store)
        }
    }
}

pub struct Connection {
    disconnector_store: DisconnectorStore,
    client: Client,
}

impl Connection {
    fn with_target(target: Target) -> Result<Self> {
        let disconnector_store: DisconnectorStore = Rc::new(Mutex::new(None));
        let disconnector_store_clone = disconnector_store.clone();
        let (client, _) = capnp_rpc::auto_reconnect(move || {
            Ok(capnp_rpc::new_future_client(open(
                target.clone(),
                disconnector_store_clone.clone(),
            )))
        })?;

        Ok(Self {
            disconnector_store,
            client,
        })
    }

    pub async fn connect(address: SocketAddr) -> Result<Self> {
        Self::with_target(Target::Plain(address))
    }

    pub async fn connect_tls(host: &str, port: u16, tls: TlsClient) -> Result<Self> {
        let server_name = ServerName::try_from(tls.server_name.clone()).map_err(|error| {
            anyhow::anyhow!("`{}` is not a server name: {error}", tls.server_name)
        })?;
        tls.config()?;
        Self::with_target(Target::Tls {
            host: host.to_string(),
            port,
            server_name,
            tls,
        })
    }

    pub async fn client(&self) -> Client {
        self.client.clone()
    }

    pub async fn disconnect(self) -> Result<()> {
        let disconnector_option = self
            .disconnector_store
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock mutex: {}", e))?
            .take();
        match disconnector_option {
            Some(disconnector) => disconnector.await.map_err(anyhow::Error::from),
            None => Ok(()), // Already disconnected
        }
    }
}
