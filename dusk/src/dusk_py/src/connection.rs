use dusk_capnp::dusk_capnp::dusk;
use dusk_connection::{CONNECT_TIMEOUT, Connection};
use dusk_program::anyhow;
use dusk_program_sh::client::shell::Shell;

pub struct Options {
    pub address: String,
    pub port: u16,
    pub sh_server_pid: u64,
}

async fn connect(host: &str, port: u16) -> anyhow::Result<Connection> {
    let no_answer = || format!("no answer within {} seconds", CONNECT_TIMEOUT.as_secs());
    let addresses: Vec<std::net::SocketAddr> =
        tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, port)))
            .await
            .map_err(|_| {
                capnp::Error::disconnected(format!("couldn't resolve `{host}`: {}", no_answer()))
            })?
            .map_err(|error| {
                capnp::Error::disconnected(format!("couldn't resolve `{host}`: {error}"))
            })?
            .collect();
    let mut refusals = Vec::new();
    for address in addresses {
        let connection = Connection::connect(address).await?;
        let answered = tokio::time::timeout(CONNECT_TIMEOUT, async {
            connection
                .client()
                .await
                .hostname_request()
                .send()
                .promise
                .await
        })
        .await
        .unwrap_or_else(|_| Err(capnp::Error::disconnected(no_answer())));
        match answered {
            Ok(_) => return Ok(connection),
            Err(error) => {
                abandon(connection).await;
                refusals.push((address, error));
            }
        }
    }
    let Some((_, last)) = refusals.last() else {
        return Err(capnp::Error::disconnected(format!("`{host}` resolves to no address")).into());
    };
    if refusals.len() == 1 {
        return Err(last.clone().into());
    }
    let refused: Vec<String> = refusals
        .iter()
        .map(|(address, error)| format!("{address}: {error}"))
        .collect();
    Err(capnp::Error {
        kind: last.kind,
        extra: format!(
            "couldn't connect to any address of `{host}`: {}",
            refused.join("; ")
        ),
    }
    .into())
}

pub async fn open(options: &Options) -> anyhow::Result<(Connection, dusk::Client, Shell)> {
    let connection = connect(&options.address, options.port).await?;
    let dusk = connection.client().await;
    let attached = async {
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
