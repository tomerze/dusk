use anyhow::Result;
use capnp_rpc::{rpc_twoparty_capnp, twoparty, Disconnector, RpcSystem};
use dusk_capnp::dusk_capnp::dusk::Client;
use futures::io::AsyncReadExt;
use std::net::SocketAddr;
use tokio::net::TcpStream;

pub struct Connection {
    disconnector: Disconnector<rpc_twoparty_capnp::Side>,
    client: Client,
}

impl Connection {
    pub async fn connect(addr: SocketAddr) -> Result<Self> {
        let stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;

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

        Ok(Self {
            disconnector,
            client,
        })
    }

    pub async fn client(&self) -> Client {
        self.client.clone()
    }

    pub async fn disconnect(self) -> Result<()> {
        self.disconnector.await.map_err(anyhow::Error::from)
    }
}
