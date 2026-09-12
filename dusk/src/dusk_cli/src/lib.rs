use anyhow::Result;
use clap::Parser;
use dusk_base::dusk_program::dusk_capnp::capnp::capability::FromClientHook as _;
use dusk_base::dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_base::dusk_program_sh::{
    ShArgs, ShMode,
    client::{prompt::stream::json_stream::JsonStream, run_prompt::Created},
    entry::StaticShEntriesBuilder,
    sh_capnp::{DEFAULT_PID, output_portal},
};
use dusk_connection::Connection;
use std::net::SocketAddr;
use std::rc::Rc;
use tokio::signal;
use tokio::sync::{Notify, oneshot};
use tracing::{debug, error, info};

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

async fn kill(client: &dusk::Client, pid: u64) {
    let mut kill_request = client.kill_request();
    kill_request.get().set_pid(pid);
    kill_request.get().set_signal(15);
    if let Err(error) = kill_request.send().promise.await {
        debug!(pid, error = %error, "kill after output returned");
    }
}

async fn prompt(client: dusk::Client, stop_signal: Rc<Notify>) -> Result<()> {
    let sh_entries_builder = StaticShEntriesBuilder::default();
    let (finished_sender, finished) = oneshot::channel();
    let program_args = ShArgs::new(client.clone(), sh_entries_builder.clone(), ShMode::Server)?
        .as_program_args()?;
    program_args.set_pid(Some(DEFAULT_PID))?;
    program_args.set_created(capnp_rpc::new_client(Created {
        client: client.clone(),
        sh_entries_builder,
        stop_signal,
        finished: Some(finished_sender),
    }))?;
    let mut process_request = client.process_request();
    program_args.with_reader(|reader| process_request.get().set_program_args(reader))?;
    let process = process_request.send().promise.await?.get()?.get_result()?;

    let mut finished = std::pin::pin!(finished);
    tokio::select! {
        result = process.run_request().send().promise => {
            if let Err(error) = result {
                debug!(error = %error, "the shell this client ran ended");
            }
        }
        _ = finished.as_mut() => return Ok(()),
    }
    let _ = finished.await;
    Ok(())
}

async fn script(client: dusk::Client, command: String, stop_signal: Rc<Notify>) -> Result<()> {
    let program_args = ShArgs::new(
        client.clone(),
        StaticShEntriesBuilder::default(),
        ShMode::Script(command),
    )?
    .as_program_args()?;
    let mut process_request = client.process_request();
    program_args.with_reader(|reader| process_request.get().set_program_args(reader))?;
    let process = process_request.send().promise.await?.get()?.get_result()?;
    let mut run_request = client.run_request();
    run_request.get().set_process(process.clone());
    run_request.send().promise.await?;

    let pid = process
        .pid_request()
        .send()
        .promise
        .await?
        .get()?
        .get_result();
    let portal = process
        .portal_request()
        .send()
        .promise
        .await?
        .get()?
        .get_result()?
        .cast_to::<output_portal::Client>();
    // Check if we are running in a terminal
    let colored = atty::is(atty::Stream::Stdout);
    let (json_stream, _done_receiver) = JsonStream::new_with_receiver(colored);
    let mut output_request = portal.output_request();
    output_request
        .get()
        .set_stream(capnp_rpc::new_client(json_stream));
    let output_promise = output_request.send().promise;
    tokio::pin!(output_promise);
    let output_reply = tokio::select! {
        reply = &mut output_promise => reply,
        _ = stop_signal.notified() => {
            kill(&client, pid).await;
            output_promise.await
        }
    };
    let daemonize = match &output_reply {
        Ok(reply) => reply.get()?.get_daemonize(),
        Err(_) => false,
    };
    if !daemonize {
        kill(&client, pid).await;
        let mut waitpid_request = client.waitpid_request();
        waitpid_request.get().set_pid(pid);
        let waited = waitpid_request.send().promise.await;
        output_reply?;
        waited?;
    }
    Ok(())
}

async fn run_sh(
    connection: &Connection,
    command: Option<String>,
    stop_signal: Rc<Notify>,
) -> Result<()> {
    let client = connection.client().await;
    match command {
        Some(command) => script(client, command, stop_signal).await,
        None => prompt(client, stop_signal).await,
    }
}

async fn stop_on_ctrl_c(stop_signal: Rc<Notify>) {
    loop {
        if signal::ctrl_c().await.is_err() {
            // SIGINT listener registration failed; park so the work arm drives shutdown.
            error!("couldn't register listener for ctrl+c");
            std::future::pending::<()>().await;
        }
        stop_signal.notify_waiters();
    }
}

async fn run(cli: Cli) {
    let local_set = tokio::task::LocalSet::new();

    if let Err(err) = local_set
        .run_until(async move {
            let connection = Connection::connect(cli.address).await?;
            let stop_signal = Rc::new(Notify::new());
            tokio::select! {
                result = run_sh(&connection, cli.command, stop_signal.clone()) => {
                    if let Err(err) = result {
                        error!("{:?}", err);
                    }
                    info!("exiting");
                }
                _ = stop_on_ctrl_c(stop_signal.clone()) => {}
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
