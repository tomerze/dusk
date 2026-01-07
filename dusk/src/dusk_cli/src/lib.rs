use anyhow::Result;
use clap::{command, Parser};
use dusk_program_sh::{
    engine::ShEngine,
    entry::{ShEntriesBuilder, StaticShEntriesBuilder},
};
use dusk_prompt::{
    display_engine::DefaultDisplayEngine,
    prompt::{Prompt, StreamRequest},
    stream::{display_stream, json_stream},
};
use dusk_shell::{connection::Connection, shell::Shell};
use std::net::SocketAddr;
use tokio::signal;
use tracing::{error, info};

#[derive(Parser)]
#[command(author, version, arg_required_else_help(true))]
struct Cli {
    #[clap(help = "address of dusk server")]
    address: SocketAddr,
    #[clap(help = "shell command to run, empty for prompt")]
    command: Option<String>,
}

async fn single_command(shell: &mut Shell, command: String) -> Result<()> {
    // Check if we are running in a terminal
    let colored = atty::is(atty::Stream::Stdout);
    let (json_stream, done_receiver) =
        dusk_prompt::stream::json_stream::JsonStream::new_with_receiver(colored);
    shell
        .sh(
            command.as_str(),
            capnp_rpc::new_client(json_stream),
            done_receiver,
        )
        .await?;
    Ok(())
}

async fn interactive_prompt(
    shell: &mut Shell,
    sh_entries_builder: impl ShEntriesBuilder,
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
    )
    .await?;
    prompt.run().await?;
    Ok(())
}

async fn run(cli: Cli) {
    let local_set = tokio::task::LocalSet::new();

    if let Err(err) = local_set
        .run_until(async move {
            let connection = Connection::connect(cli.address).await?;
            tokio::select! {
                result = async {
                    let client = connection.client().await;
                    let sh_entries_builder = StaticShEntriesBuilder::default();
                    let mut shell = Shell::new(
                            ShEngine::new(
                                client,
                                sh_entries_builder.clone()
                            )
                        ).await?;
                    let session_result = match cli.command {
                        Some(command) => single_command(&mut shell, command).await,
                        None => interactive_prompt(&mut shell, sh_entries_builder).await,
                    };
                    let shell_kill_result = shell.kill().await;
                    // Print both shell kill errors and command errors
                    if let Err(err) = shell_kill_result {
                        error!("error killing shell: {}", err);
                    }
                    if let Err(err) = session_result {
                        error!("{}", err);
                    }
                    Ok::<(), anyhow::Error>(())
                } => {
                    result?;
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
        // Exit with error
        std::process::exit(1);
    }
}

#[tokio::main]
pub async fn main() -> Result<()> {
    console_subscriber::init();

    let cli = Cli::parse_from(argfile::expand_args_from(
        wild::args_os(),
        argfile::parse_fromfile,
        argfile::PREFIX,
    )?);

    info!("connecting to {}", cli.address);
    run(cli).await;

    Ok(())
}
