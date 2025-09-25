use anyhow::Result;
use clap::{command, Parser};
use dusk_program_sh::program_args_builder::StaticProgramArgsBuilder;
use dusk_prompt::{connection::Connection, prompt::Prompt, shell::Shell};
use std::net::SocketAddr;
use tokio::signal;
use tracing::{debug, error, info};

#[derive(Parser)]
#[command(author, version, arg_required_else_help(true))]
struct Cli {
    #[clap(help = "address of dusk server")]
    address: SocketAddr,
}

async fn run(address: SocketAddr) {
    async fn inner(connection: &Connection) -> Result<()> {
        let client = connection.client().await;
        let program_args_builder = StaticProgramArgsBuilder::new(client.clone());
        let shell = Shell::new(client, program_args_builder).await?;
        let prompt = Prompt::new(shell).await?;
        prompt.run().await?;

        Ok(())
    }
    let local_set = tokio::task::LocalSet::new();

    if let Err(err) = local_set
        .run_until(async move {
            info!("connecting to {}", address);
            let connection = Connection::connect(address).await?;
            debug!("connected to {}", address);
            if let Err(err) = inner(&connection).await {
                error!("interactive prompt failed: `{err}`");
            }
            connection.disconnect().await?;
            Ok::<(), anyhow::Error>(())
        })
        .await
    {
        error!("connection error: {}", err);
    }
    local_set.await;
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
