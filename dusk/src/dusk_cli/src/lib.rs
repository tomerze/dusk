use anyhow::Result;
use clap::Parser;
use dusk_base::dusk_program::dusk_capnp::capnp::capability::FromClientHook as _;
use dusk_base::dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_base::dusk_program_sh::{
    ShArgs, ShMode,
    client::{
        open_prompt,
        prompt::stream::json_stream::JsonStream,
        stop::{StopSignal, stop_innermost},
    },
    sh_capnp::{DEFAULT_PID, output_portal},
};
use dusk_connection::Connection;
use std::net::SocketAddr;
use tokio::signal;
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

async fn script(client: dusk::Client, command: String) -> Result<()> {
    let stop_signal = StopSignal::new();
    let script = dusk_base::dusk_program_sh::compile(client.clone(), &command).await?;
    let program_args = ShArgs::new(ShMode::Script(script))?.as_program_args()?;
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
    let stop = stop_signal.signal();
    let output_reply = tokio::select! {
        reply = &mut output_promise => reply,
        _ = stop.notified() => {
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

async fn run_sh(connection: &Connection, command: Option<String>) -> Result<()> {
    let client = connection.client().await;
    match command {
        Some(command) => script(client, command).await,
        None => open_prompt(client, DEFAULT_PID).await,
    }
}

async fn stop_on_ctrl_c() {
    loop {
        if signal::ctrl_c().await.is_err() {
            // SIGINT listener registration failed; park so the work arm drives shutdown.
            error!("couldn't register listener for ctrl+c");
            std::future::pending::<()>().await;
        }
        stop_innermost();
    }
}

async fn run(cli: Cli) {
    let local_set = tokio::task::LocalSet::new();

    if let Err(err) = local_set
        .run_until(async move {
            let connection = Connection::connect(cli.address).await?;
            tokio::select! {
                result = run_sh(&connection, cli.command) => {
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
        // command's stdout - e.g. `logs --replay-only` stays a clean dump.
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(env_filter)
            .init();
    }

    info!("attempting to connect to {}", cli.address);
    run(cli).await;

    std::process::exit(0);
}
