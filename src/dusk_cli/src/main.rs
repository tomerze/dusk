use anyhow::Result;
use chrono::Duration;
use clap::{command, Parser};
use crossterm::{event::DisableBracketedPaste, execute};
use reedline::{Reedline, Signal};
use std::io::stdout;
use std::net::SocketAddr;
use tokio::signal;
use tracing::{error, info};

mod prompt;

#[derive(Parser)]
#[command(author, version, arg_required_else_help(true))]
struct Cli {
    #[clap(help = "address of dusk server")]
    address: SocketAddr,
}

async fn run_interactive_shell() -> Result<()> {
    // TODO: get this info after connecting to dusk server
    let mut line_editor = prompt::get_line_editor(vec![
        "dusk".into(),
        "help".into(),
        "clear".into(),
        "exit".into(),
        "quit".into(),
    ])?;

    let prompt = prompt::DuskPrompt::new("Dusk");

    loop {
        let sig = line_editor.read_line(&prompt)?;
        match sig {
            Signal::Success(buffer) => {
                if !buffer.is_empty() {
                    line_editor.update_last_command_context(
                        &|mut history_item: reedline::HistoryItem| {
                            history_item.start_timestamp = Some(chrono::Utc::now());
                            history_item
                        },
                    )?;
                }
                let start_timestamp = std::time::Instant::now();

                process_line(&buffer, &mut line_editor).await?;

                let duration = start_timestamp.elapsed();
                prompt.right_prompt.set(Duration::from_std(duration)?);
                if !buffer.is_empty() {
                    line_editor.update_last_command_context(&|mut history_item| {
                        history_item.duration = Some(duration);
                        history_item.exit_status = Some(0);
                        history_item
                    })?;
                }
            }
            Signal::CtrlD | Signal::CtrlC => {
                info!("aborted");
                break;
            }
        }
    }

    execute!(stdout(), DisableBracketedPaste)?;

    Ok(())
}

async fn process_command(program: &str, args: Vec<&str>) {
    info!("run program {program} with args {args:?}");
}

async fn process_line(line: &str, line_editor: &mut Reedline) -> Result<()> {
    let mut args = line.split_ascii_whitespace();

    match args.next() {
        Some("exit") => {
            std::process::exit(0);
        }
        Some("quit") => std::process::exit(0),
        Some("clear") => {
            line_editor.clear_screen()?;
        }
        Some(program) => {
            process_command(program, args.collect()).await;
        }
        None => {}
    };

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
