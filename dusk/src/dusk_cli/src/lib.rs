use anyhow::Result;
use clap::Parser;
use dusk_base::dusk_program_sh::{
    Execution, ShArgs, ShMode, Stop,
    client::{
        prompt::{serving, stream::json_stream::JsonStream, wait_serving},
        shell::stop::{StopScope, stop_innermost},
    },
    entry::StaticShEntriesBuilder,
};
use dusk_connection::Connection;
use std::net::SocketAddr;
use std::rc::Rc;
use tokio::signal;
use tokio::sync::Notify;
use tracing::{error, info};

#[derive(Parser)]
#[command(author, version, arg_required_else_help(true))]
struct Cli {
    #[clap(help = "address of dusk server")]
    address: SocketAddr,
    #[clap(help = "shell command to run, empty for prompt")]
    command: Option<String>,
    #[clap(long, help = "enable tokio console debugging")]
    debug_console: bool,
}

async fn run_sh(
    connection: &Connection,
    command: Option<String>,
    stop_signal: Rc<Notify>,
) -> Result<()> {
    let client = connection.client().await;
    let mode = match command {
        Some(command) => ShMode::Script(command),
        None => ShMode::Server,
    };
    let program_args =
        ShArgs::new(client.clone(), StaticShEntriesBuilder::default(), mode)?.as_program_args()?;

    let colored = atty::is(atty::Stream::Stdout);
    let (json_stream, _done_receiver) = JsonStream::new_with_receiver(colored);
    let execution = Execution::new(client, capnp_rpc::new_client(json_stream));

    let _stop_scope = StopScope::enter(stop_signal.clone());
    let serving_before = serving();
    let stop = Stop::new();
    let mut notified = std::pin::pin!(stop_signal.notified());
    notified.as_mut().enable();
    let mut execution_future = std::pin::pin!(execution.program_args(program_args, &stop));
    let result = tokio::select! {
        result = &mut execution_future => result,
        _ = &mut notified => {
            stop.signal(());
            execution_future.await
        }
    };
    wait_serving(serving_before).await;
    Ok(result?)
}

async fn stop_on_ctrl_c() {
    loop {
        if signal::ctrl_c().await.is_err() {
            // SIGINT listener registration failed; park so the work arm drives shutdown.
            error!("couldn't register listener for ctrl+c");
            std::future::pending::<()>().await;
        }
        if !stop_innermost() {
            tracing::warn!("ctrl+c with nothing to stop");
        }
    }
}

async fn run(cli: Cli) {
    let local_set = tokio::task::LocalSet::new();

    if let Err(err) = local_set
        .run_until(async move {
            let connection = Connection::connect(cli.address).await?;
            let stop_signal = Rc::new(Notify::new());
            tokio::select! {
                result = run_sh(&connection, cli.command, stop_signal) => {
                    if let Err(err) = result {
                        error!("{:?}", err);
                    }
                    info!("exiting");
                }
                _ = stop_on_ctrl_c() => {}
            };
            connection.disconnect().await?;
            Ok::<(), anyhow::Error>(())
        })
        .await
    {
        error!("critical error: {:?}", err);
        // Exit with error
        std::process::exit(1);
    }
}

#[tokio::main]
pub async fn main() -> Result<()> {
    let cli = Cli::parse_from(argfile::expand_args_from(
        wild::args_os(),
        argfile::parse_fromfile,
        argfile::PREFIX,
    )?);

    if cli.debug_console {
        console_subscriber::init();
    } else {
        use tracing_subscriber::EnvFilter;

        let env_filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        // The CLI's own diagnostics go to stderr so they never mix into a
        // command's stdout — e.g. `logs --replay-only` stays a clean dump.
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(env_filter)
            .init();
    }

    info!("connecting to {}", cli.address);
    run(cli).await;

    std::process::exit(0);
}
