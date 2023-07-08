use anyhow::Result;
use clap::{command, Parser};
use rustyline::DefaultEditor;
use shellfish::{handler::DefaultAsyncHandler, Shell};
use std::net::SocketAddr;
use tokio::signal;
use tracing::{error, info};
use colored::Colorize;

#[derive(Parser)]
#[command(author, version, arg_required_else_help(true))]
struct Cli {
    #[clap(help = "address of dusk server")]
    address: SocketAddr,
}

async fn run_interactive_shell() -> Result<()> {
    // Define a shell
    let mut shell = Shell::new_with_async_handler(
        0_u64,
        format!("{}{}{}{}", "[".bright_yellow(), "Dusk".bright_blue(), "]".bright_yellow(), " # ".bright_red()),
        DefaultAsyncHandler::default(),
        DefaultEditor::new()?,
    );

    shell.run_async().await?;
    Ok(())
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
        _ = async move {
                info!("connecting to {}", cli.address);
                run_interactive_shell().await.unwrap();
            } => {
            info!("exiting");
        }
        _ = signal::ctrl_c() => {
            error!("existing due to signal");
        }
    };

    Ok(())
}
