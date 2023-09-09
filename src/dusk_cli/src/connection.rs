use anyhow::Result;
use capnp_rpc::{rpc_twoparty_capnp, twoparty, RpcSystem};
use dusk::dusk_capnp;
use futures::io::AsyncReadExt;
use std::net::SocketAddr;
use tokio::net::TcpStream;

pub struct Connection {
    stream: TcpStream,
}

impl Connection {
    pub async fn connect(addr: SocketAddr) -> Result<Self> {
        let stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;

        Ok(Self { stream })
    }

    pub async fn client(self) -> dusk_capnp::dusk::Client {
        let (reader, writer) =
            tokio_util::compat::TokioAsyncReadCompatExt::compat(self.stream).split();
        let rpc_network = Box::new(twoparty::VatNetwork::new(
            reader,
            writer,
            rpc_twoparty_capnp::Side::Client,
            Default::default(),
        ));
        let mut rpc_system = RpcSystem::new(rpc_network, None);
        let dusk_client: dusk_capnp::dusk::Client =
            rpc_system.bootstrap(rpc_twoparty_capnp::Side::Server);
        tokio::task::spawn_local(rpc_system);

        dusk_client
    }
}
