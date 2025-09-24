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
    ) -> capnp::Result<(Client, Disconnector<rpc_twoparty_capnp::Side>)> {
        let std_stream = std::net::TcpStream::connect(addr)?;
        std_stream.set_nodelay(true)?;
        std_stream.set_nonblocking(true)?;

        let stream = TcpStream::from_std(std_stream)?;
        let (reader, writer) = tokio_util::compat::TokioAsyncReadCompatExt::compat(stream).split();
        let rpc_network = Box::new(twoparty::VatNetwork::new(
            reader,
            writer,
            rpc_twoparty_capnp::Side::Client,
            Default::default(),
        ));
        let mut rpc_system = RpcSystem::new(rpc_network, None);
        let disconnector = rpc_system.get_disconnector();
        let client: Client = rpc_system.bootstrap(rpc_twoparty_capnp::Side::Server);
        tokio::task::spawn_local(rpc_system);
        Ok((client, disconnector))
    }

    pub async fn connect(addr: SocketAddr) -> Result<Self> {
        let disconnector_store: DisconnectorStore = Rc::new(Mutex::new(None));
        let disconnector_store_clone = disconnector_store.clone();

        let (client, _) = capnp_rpc::auto_reconnect(move || {
            let disconnector_store_clone = disconnector_store_clone.clone();

            if let Ok((client, disconnector)) = Self::auto_connect(addr) {
                disconnector_store_clone
                    .lock()
                    .map_err(|e| capnp::Error::failed(format!("Failed to lock mutex: {}", e)))
                    .map(|mut guard| {
                        *guard = Some(disconnector);
                        client
                    })
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
