use anyhow::Result;
use capnp_rpc::{Disconnector, RpcSystem, rpc_twoparty_capnp, twoparty};
use dusk_capnp::dusk_capnp::dusk::Client;
use futures::io::AsyncReadExt;
use std::time::Duration;
use std::{net::SocketAddr, rc::Rc, sync::Mutex};
use tokio::net::TcpStream;

type DisconnectorStore = Rc<Mutex<Option<Disconnector<rpc_twoparty_capnp::Side>>>>;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Connection {
    disconnector_store: DisconnectorStore,
    client: Client,
}

impl Connection {
    fn auto_connect(
        address: SocketAddr,
        disconnector_store: DisconnectorStore,
    ) -> capnp::Result<Client> {
        let client = capnp_rpc::new_future_client(async move {
            let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address))
                .await
                .map_err(|_| {
                    capnp::Error::disconnected(format!(
                        "couldn't connect to {address}: no answer within {} seconds",
                        CONNECT_TIMEOUT.as_secs()
                    ))
                })??;
            stream.set_nodelay(true)?;

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
                    .map_err(|e| capnp::Error::failed(e.to_string()))?;
                *disconnector_option_guard = Some(disconnector);
            }

            let client: Client = rpc_system.bootstrap(rpc_twoparty_capnp::Side::Server);
            tokio::task::spawn_local(rpc_system);
            Ok(client)
        });
        Ok(client)
    }

    pub async fn connect(address: SocketAddr) -> Result<Self> {
        let disconnector_store: DisconnectorStore = Rc::new(Mutex::new(None));
        let disconnector_store_clone = disconnector_store.clone();
        let (client, _) = capnp_rpc::auto_reconnect(move || {
            if let Ok(client) = Self::auto_connect(address, disconnector_store_clone.clone()) {
                Ok(client)
            } else {
                Err(capnp::Error::failed("Failed to connect".to_string()))
            }
        })?;

        Ok(Self {
            disconnector_store,
            client,
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
