use anyhow::Result;
use clap::{command, Parser};
use dusk_prompt::{connection::Connection, prompt::Prompt, shell::Shell};
use std::net::SocketAddr;
use tokio::signal;
use tracing::{error, info, debug};

#[derive(Parser)]
#[command(author, version, arg_required_else_help(true))]
struct Cli {
    #[clap(help = "address of dusk server")]
    address: SocketAddr,
}

async fn run(address: SocketAddr) {
    async fn inner(address: SocketAddr) -> Result<()> {
        info!("connecting to {}", address);
        let client = Connection::connect(address).await?.client().await;
        debug!("connected to {}", address);

        Prompt::new(Shell::new(client).await?).await?.run().await?;

        Ok(())
    }
    tokio::task::LocalSet::new()
        .run_until(async move {
            if let Err(err) = inner(address).await {
                error!("interactive prompt failed: `{err}`");
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
        _ = run(cli.address) => {
            info!("exiting");
        }
        _ = signal::ctrl_c() => {
            error!("existing due to signal");
        }
    };

    Ok(())
}
