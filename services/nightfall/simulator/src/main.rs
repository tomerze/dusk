use anyhow::Context;
use clap::Parser;
use nightfall_simulator::fleet::Fleet;
use nightfall_simulator::{identity, link};
use rustls_pki_types::ServerName;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "nightfall_simulator",
    version,
    about = "Simulate many dusk nodes, each holding its own fleet link to nightfall"
)]
struct Command {
    #[arg(
        long,
        help = "The fleet address of nightfall, host:port, resolved again every 30 s and after a failed connection"
    )]
    fleet: String,
    #[arg(
        long,
        default_value = "fleet.dusk.test",
        help = "The server name nightfall's fleet certificate carries"
    )]
    server_name: String,
    #[arg(
        long,
        help = "The fleet-server CA that nightfall's fleet certificate chains to"
    )]
    fleet_server_ca: PathBuf,
    #[arg(
        long,
        help = "The CA certificate the links' certificates are signed with; nightfall must trust it as a fleet client CA"
    )]
    issuer_certificate: PathBuf,
    #[arg(long, help = "The unencrypted private key of that CA")]
    issuer_key: PathBuf,
    #[arg(long, help = "The tenant the links' certificates name")]
    tenant: Option<String>,
    #[arg(
        long,
        default_value_t = 168,
        help = "Hours each link's certificate is valid for"
    )]
    certificate_lifetime_hours: u64,
    #[arg(
        long,
        default_value_t = 1000,
        help = "How many links this process holds"
    )]
    links: u64,
    #[arg(
        long,
        default_value_t = 0,
        help = "This process's index; links of different indexes are different nodes"
    )]
    simulator: u64,
    #[arg(
        long,
        default_value_t = 100.0,
        help = "How many links start each second"
    )]
    links_per_second: f64,
    #[arg(
        long,
        help = "Threads that hold the links (default: one per available CPU)"
    )]
    threads: Option<usize>,
    #[arg(
        long,
        default_value_t = 90,
        help = "Seconds without a heartbeat from nightfall before a link is closed"
    )]
    heartbeat_timeout_seconds: u64,
    #[arg(
        long,
        default_value = "0.0.0.0:9103",
        help = "Where Prometheus metrics are served"
    )]
    metrics_listen: SocketAddr,
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("NIGHTFALL_SIMULATOR_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_current_span(false)
        .with_target(true)
        .init();
}

fn raise_descriptor_limit() -> std::io::Result<u64> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if limit.rlim_cur < limit.rlim_max {
        limit.rlim_cur = limit.rlim_max;
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(limit.rlim_cur)
}

fn hold(
    settings: Arc<link::Settings>,
    thread: u64,
    threads: u64,
    links: u64,
    links_per_second: f64,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a thread's runtime");
    let local = tokio::task::LocalSet::new();
    let origin = tokio::time::Instant::now();
    local.block_on(&runtime, async move {
        for link in (thread..links).step_by(threads as usize) {
            let start_at = origin + Duration::from_secs_f64(link as f64 / links_per_second);
            tokio::task::spawn_local(link::run(settings.clone(), link, start_at));
        }
        std::future::pending::<()>().await
    });
}

fn simulate(command: Command) -> anyhow::Result<ExitCode> {
    anyhow::ensure!(
        command.links_per_second > 0.0 && command.links_per_second.is_finite(),
        "--links-per-second must be a positive number"
    );
    let descriptors = raise_descriptor_limit().context("raise the open file limit")?;
    if descriptors < command.links + 1024 {
        tracing::warn!(
            links = command.links,
            open_file_limit = descriptors,
            "the open file limit is below the links this process holds"
        );
    }
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .with_http_listener(command.metrics_listen)
        .install()
        .context("serve the metrics")?;
    metrics::gauge!("nightfall_simulator_links_configured").set(command.links as f64);
    let server_name = ServerName::try_from(command.server_name.clone())
        .with_context(|| format!("{} is not a server name", command.server_name))?;
    let issuer = identity::Issuer::load(
        &command.issuer_certificate,
        &command.issuer_key,
        &command.fleet_server_ca,
        Duration::from_secs(command.certificate_lifetime_hours.saturating_mul(3600)),
        command.tenant.clone(),
    )?;
    let first = identity::device_id(command.simulator, 0);
    issuer
        .mint(&first, &identity::installation_id(&first))
        .context("mint a link certificate")?;
    let threads = command
        .threads
        .or_else(|| std::thread::available_parallelism().ok().map(usize::from))
        .unwrap_or(1)
        .max(1) as u64;
    tracing::info!(
        fleet = %command.fleet,
        server_name = %command.server_name,
        links = command.links,
        simulator = command.simulator,
        links_per_second = command.links_per_second,
        threads,
        open_file_limit = descriptors,
        metrics_listen = %command.metrics_listen,
        "simulating dusk nodes"
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build the main runtime")?;
    let fleet = Fleet::new(command.fleet);
    runtime.block_on(fleet.resolve());
    let settings = Arc::new(link::Settings {
        fleet: fleet.clone(),
        server_name,
        issuer,
        simulator: command.simulator,
        heartbeat_timeout: Duration::from_secs(command.heartbeat_timeout_seconds),
    });
    for thread in 0..threads {
        let settings = settings.clone();
        let links = command.links;
        let links_per_second = command.links_per_second;
        std::thread::Builder::new()
            .name(format!("links-{thread}"))
            .spawn(move || hold(settings, thread, threads, links, links_per_second))
            .context("start a thread")?;
    }
    runtime
        .block_on(async {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            tokio::select! {
                _ = terminate.recv() => {}
                _ = tokio::signal::ctrl_c() => {}
                () = fleet.keep_resolved() => {}
            }
            Ok::<(), std::io::Error>(())
        })
        .context("wait for a signal")?;
    tracing::info!("stopping: every link closes as the process exits");
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let command = Command::parse();
    init_logging();
    match simulate(command) {
        Ok(code) => code,
        Err(error) => {
            tracing::error!(error = format!("{error:#}"), "the simulator failed");
            eprintln!("nightfall_simulator: {error:#}");
            ExitCode::FAILURE
        }
    }
}
