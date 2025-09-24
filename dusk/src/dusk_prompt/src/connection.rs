use anyhow::Result;
use capnp_rpc::{rpc_twoparty_capnp, twoparty, Disconnector, RpcSystem};
use dusk_capnp::dusk_capnp::dusk::Client;
use futures::io::AsyncReadExt;
use std::{net::SocketAddr, rc::Rc, sync::Mutex};
use tokio::net::TcpStream;

type DisconnectorStore = Rc<Mutex<Option<Disconnector<rpc_twoparty_capnp::Side>>>>;

pub struct Connection {
    disconnector_store: DisconnectorStore,
    client: Client,
}

impl Connection {
    fn auto_connect(
        addr: SocketAddr,
        disconnector_store: DisconnectorStore,
    ) -> capnp::Result<Client> {
        let client = capnp_rpc::new_future_client(async move {
            let stream = TcpStream::connect(addr).await?;
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

    pub async fn connect(addr: SocketAddr) -> Result<Self> {
        let disconnector_store: DisconnectorStore = Rc::new(Mutex::new(None));
        let disconnector_store_clone = disconnector_store.clone();
        let (client, _) = capnp_rpc::auto_reconnect(move || {
            if let Ok(client) = Self::auto_connect(addr, disconnector_store_clone.clone()) {
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
        let disconnector = self
            .disconnector_store
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock mutex: {}", e))?
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to get disconnector"))?;
        disconnector.await.map_err(anyhow::Error::from)
    }
}
