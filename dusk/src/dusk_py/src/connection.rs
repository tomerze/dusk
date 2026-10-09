use dusk_capnp::dusk_capnp::dusk;
use dusk_connection::Connection;
use dusk_program::anyhow;
use dusk_program_sh::client::shell::Shell;

pub struct Options {
    pub address: std::net::SocketAddr,
    pub sh_server_pid: u64,
}

pub async fn open(options: &Options) -> anyhow::Result<(Connection, dusk::Client, Shell)> {
    let connection = Connection::connect(options.address).await?;
    let dusk = connection.client().await;
    let attached = async {
        dusk.hostname_request().send().promise.await?;
        let process = Shell::recreate_sh_process(dusk.clone(), options.sh_server_pid).await?;
        Shell::new(dusk.clone(), process).await
    }
    .await;
    match attached {
        Ok(shell) => Ok((connection, dusk, shell)),
        Err(error) => {
            abandon(connection).await;
            Err(error)
        }
    }
}

async fn abandon(connection: Connection) {
    if let Err(error) = connection.disconnect().await {
        tracing::warn!(
            error = %format!("{error:#}"),
            "couldn't disconnect after the connection failed to set up"
        );
    }
}
