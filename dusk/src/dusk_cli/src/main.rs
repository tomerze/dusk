use anyhow::Result;
use clap::{command, Parser};
use dusk_program_sh::{engine::ShEngine, entry::StaticShEntriesBuilder};
use dusk_prompt::{
    connection::Connection, display_engine::DisplayEngineImpl, prompt::Prompt, shell::Shell,
};
use std::net::SocketAddr;
use tokio::signal;
use tracing::{error, info};

#[derive(Parser)]
#[command(author, version, arg_required_else_help(true))]
struct Cli {
    #[clap(help = "address of dusk server")]
    address: SocketAddr,
}

async fn run(address: &SocketAddr) {
    let local_set = tokio::task::LocalSet::new();

    if let Err(err) = local_set
        .run_until(async move {
            let connection = Connection::connect(*address).await?;
            tokio::select! {
                _ = async {
                    let client = connection.client().await;
                    let sh_entries_builder = StaticShEntriesBuilder::default();
                    let prompt = Prompt::new(
                        Shell::new(
                            ShEngine::new(
                                client,
                                sh_entries_builder.clone()
                            )
                        ).await?,
                        sh_entries_builder,
                        DisplayEngineImpl::default()
                    ).await?;
                    prompt.run().await?;
                    Ok::<(), anyhow::Error>(())
                } => {
                    info!("exiting");
                }
                _ = signal::ctrl_c() => {
                    error!("existing due to signal not caught by prompt");
                }
            };
            connection.disconnect().await?;
            Ok::<(), anyhow::Error>(())
        })
        .await
    {
        error!("critical error: {}", err);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    console_subscriber::init();

    let cli = Cli::parse_from(argfile::expand_args_from(
        wild::args_os(),
        argfile::parse_fromfile,
        argfile::PREFIX,
    )?);

    info!("connecting to {}", cli.address);
    run(&cli.address).await;

    Ok(())
}
