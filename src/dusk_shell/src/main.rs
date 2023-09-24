use anyhow::Result;
use clap::{command, Parser};
use std::net::SocketAddr;
use tokio::signal;
use tracing::{error, info};

mod connection;
mod prompt;
mod shell;

#[derive(Parser)]
#[command(author, version, arg_required_else_help(true))]
struct Cli {
    #[clap(help = "address of dusk server")]
    address: SocketAddr,
}

async fn run_interactive(address: SocketAddr) {
    async fn inner(address: SocketAddr) -> Result<()> {
        info!("connecting to {}", address);
        let client = connection::Connection::connect(address)
            .await?
            .client()
            .await;

        let reply = client.hostname_request().send().promise.await?;
        let hostname = reply.get()?.get_hostname()?.to_str()?;

        shell::Shell::new(hostname, vec![]).run().await.unwrap();
        Ok(())
    }
    tokio::task::LocalSet::new()
        .run_until(async move {
            if let Err(err) = inner(address).await {
                error!("interactive shell failed: `{err}`");
            }
        })
        .await;
}

#[tokio::main]
async fn main() -> Result<()> {
    console_subscriber::init();

    let cli = Cli::parse_from(argfile::expand_args_from(
        wild::args_os(),
        argfile::parse_fromfile,
        argfile::PREFIX,
    )?);

    tokio::select! {
        _ = run_interactive(cli.address) => {
            info!("exiting");
        }
        _ = signal::ctrl_c() => {
            error!("existing due to signal");
        }
    };

    Ok(())
}
