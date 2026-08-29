use anyhow::Result;
use clap::Parser;
use dusk_base::dusk_program_sh::{
    client::{
        prompt::{
            Prompt, StreamRequest,
            display_engine::DefaultDisplayEngine,
            stream::{display_stream, json_stream},
        },
        shell::{Shell, stop::stop_innermost},
    },
    entry::{ShEntriesBuilder, StaticShEntriesBuilder},
    parser::Parser as ShParser,
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

async fn single_command(shell: &mut Shell, command: String, stop_signal: Rc<Notify>) -> Result<()> {
    // Check if we are running in a terminal
    let colored = atty::is(atty::Stream::Stdout);
    let (json_stream, done_receiver) = json_stream::JsonStream::new_with_receiver(colored);
    shell
        .sh(
            command.as_str(),
            capnp_rpc::new_client(json_stream),
            done_receiver,
            stop_signal,
        )
        .await?;
    Ok(())
}

async fn interactive_prompt(
    shell: &mut Shell,
    sh_entries_builder: impl ShEntriesBuilder,
    stop_signal: Rc<Notify>,
) -> Result<()> {
    let stream_factory = |request: StreamRequest<DefaultDisplayEngine>| match request {
        StreamRequest::Raw => {
            let (json_stream, done_receiver) = json_stream::JsonStream::new_with_receiver(true);
            let json_stream = capnp_rpc::new_client(json_stream);
            (json_stream, done_receiver)
        }
        StreamRequest::Display { display_engine } => {
            let (display_stream, done_receiver) =
                display_stream::DisplayStream::new_with_receiver(display_engine.clone());
            let display_stream = capnp_rpc::new_client(display_stream);
            (display_stream, done_receiver)
        }
    };

    let prompt = Prompt::new(
        shell,
        sh_entries_builder,
        DefaultDisplayEngine::default(),
        stream_factory,
        stop_signal,
    )
    .await?;
    prompt.run().await?;
    Ok(())
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
                result = async {
                    let client = connection.client().await;
                    let sh_entries_builder = StaticShEntriesBuilder::default();
                    let mut shell = Shell::new(
                        client.clone(),
                        sh_entries_builder.clone(),
                        ShParser::new(),
                    )
                    .await?;
                    let session_result = match cli.command {
                        Some(command) => {
                            single_command(&mut shell, command, stop_signal.clone()).await
                        }
                        None => {
                            interactive_prompt(&mut shell, sh_entries_builder, stop_signal.clone())
                                .await
                        }
                    };
                    let shell_kill_result = shell.kill().await;
                    // Print both shell kill errors and command errors
                    if let Err(err) = shell_kill_result {
                        error!("error killing shell: {:?}", err);
                    }
                    if let Err(err) = session_result {
                        error!("{:?}", err);
                    }
                    Ok::<(), anyhow::Error>(())
                } => {
                    result?;
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
