use crate::ctrl_c::stop_on_ctrl_c;
use crate::shell_output::{self, OutputSender};
use dusk_capnp::dusk_capnp::dusk;
use dusk_connection::{CONNECT_TIMEOUT, Connection, TlsClient};
use dusk_program::anyhow::{self, anyhow};
use dusk_program_sh::client::open_prompt;
use dusk_program_sh::client::shell::Shell;
use std::rc::Rc;
use tokio::sync::{Mutex as TokioMutex, mpsc, oneshot};
use tracing::Instrument as _;

pub const COMMAND_QUEUE: usize = 64;

const ATTACH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

pub type Reply<Answer> = oneshot::Sender<anyhow::Result<Answer>>;

pub enum Command {
    Sh {
        command: String,
        output: OutputSender,
    },
    Prompt {
        reply: Reply<()>,
    },
}

pub struct Options {
    pub address: String,
    pub port: u16,
    pub sh_server_pid: u64,
    pub tls: Option<TlsClient>,
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

async fn open(options: &Options) -> anyhow::Result<(Connection, dusk::Client, Shell)> {
    let connection = match &options.tls {
        None => connect(&options.address, options.port).await?,
        Some(tls) => Connection::connect_tls(&options.address, options.port, tls.clone()).await?,
    };
    let dusk = connection.client().await;
    let attached = tokio::time::timeout(ATTACH_TIMEOUT, async {
        let process = Shell::recreate_sh_process(dusk.clone(), options.sh_server_pid).await?;
        Shell::new(dusk.clone(), process).await
    })
    .await
    .unwrap_or_else(|_| {
        Err(capnp::Error::disconnected(format!(
            "couldn't attach to the shell server: no answer within {} seconds",
            ATTACH_TIMEOUT.as_secs()
        ))
        .into())
    });
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

fn refuse_reply<Answer>(reply: Reply<Answer>) {
    if reply.send(Err(anyhow!("Connection is closed"))).is_err() {
        tracing::warn!("nobody was waiting for a command the closed connection refused");
    }
}

async fn refuse(command: Command) {
    match command {
        Command::Sh { output, .. } => output.fail(anyhow!("Connection is closed")).await,
        Command::Prompt { reply } => refuse_reply(reply),
    }
}

pub async fn serve(
    options: Options,
    mut commands: mpsc::Receiver<Command>,
    mut shutdown: oneshot::Receiver<Reply<()>>,
    ready: Reply<()>,
) {
    let span = tracing::info_span!(
        "connection",
        address = options.address.as_str(),
        port = options.port,
        sh_server_pid = options.sh_server_pid,
        tls = options.tls.is_some()
    );
    async move {
        let (connection, dusk, shell) = match open(&options).await {
            Ok(opened) => opened,
            Err(error) => {
                tracing::info!(error = %format!("{error:#}"), "couldn't connect");
                if ready.send(Err(error)).is_err() {
                    tracing::warn!("nobody was waiting for the connection");
                }
                return;
            }
        };
        tracing::info!("connected");
        let shell = Rc::new(TokioMutex::new(shell));
        let mut running = tokio::task::JoinSet::new();
        let reply = if ready.send(Ok(())).is_err() {
            tracing::warn!("nobody was waiting for the connection, it closes");
            None
        } else {
            loop {
                tokio::select! {
                    Some(finished) = running.join_next() => {
                        if let Err(error) = finished {
                            tracing::error!(error = %error, "a command's task panicked");
                        }
                    }
                    requested = &mut shutdown => break requested.ok(),
                    command = commands.recv(), if running.len() < COMMAND_QUEUE => match command {
                        Some(Command::Sh { command, output }) => {
                            running.spawn_local(shell_output::run(shell.clone(), command, output));
                        }
                        Some(Command::Prompt { reply }) => {
                            let dusk = dusk.clone();
                            let sh_server_pid = options.sh_server_pid;
                            running.spawn_local(async move {
                                let prompted = stop_on_ctrl_c(open_prompt(dusk, sh_server_pid)).await;
                                if reply.send(prompted).is_err() {
                                    tracing::warn!("nobody was waiting for the prompt's result");
                                }
                            });
                        }
                        None => break None,
                    },
                }
            }
        };
        if !running.is_empty() {
            tracing::info!(commands = running.len(), "stopping the commands still running");
        }
        running.abort_all();
        while let Some(finished) = running.join_next().await {
            if let Err(error) = finished
                && error.is_panic()
            {
                tracing::error!(error = %error, "a command's task panicked");
            }
        }
        commands.close();
        while let Ok(command) = commands.try_recv() {
            refuse(command).await;
        }
        drop(shell);
        drop(dusk);
        let shutdown = connection.disconnect().await;
        match reply {
            Some(reply) => {
                if let Err(Err(error)) = reply.send(shutdown) {
                    tracing::warn!(error = %format!("{error:#}"), "couldn't disconnect, and nobody was waiting to hear it");
                }
            }
            None => {
                if let Err(error) = shutdown {
                    tracing::warn!(error = %format!("{error:#}"), "couldn't disconnect");
                }
            }
        }
        tracing::info!("disconnected");
    }
    .instrument(span)
    .await
}
