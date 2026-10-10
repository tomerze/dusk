use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use nightfall::config::{Config, DEFAULT_PATH};
use nightfall::kafka::KafkaBroker;
use nightfall::server::Services;
use nightfall_ledger::kafka::{KafkaLog, KafkaLogConfig, PartitionRange, verify_partition};
use nightfall_ledger::param_hash::ParamKey;
use nightfall_ledger::signing::VerifyingKeys;
use nightfall_ledger::verifier::{VerifyOptions, verify_lines};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "nightfall",
    version,
    about = "The security layer of the Dusk stack: the front door for nodes and the clients that drive them"
)]
struct Command {
    #[arg(long, global = true, default_value = DEFAULT_PATH, help = "The configuration file")]
    config: PathBuf,
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    #[command(about = "Run nightfall (the default)")]
    Serve,
    #[command(about = "Verify ledger chains and checkpoints from JSON lines or a Kafka partition")]
    VerifyLedger(VerifyLedger),
    #[command(about = "Recompute the param_hash of canonical call parameters")]
    LedgerHash(LedgerHash),
}

#[derive(Args)]
struct VerifyLedger {
    #[arg(long, help = "The JWKS file holding the checkpoint public keys")]
    keys: PathBuf,
    #[arg(
        long,
        help = "A file of ledger entries as JSON lines, or - for standard input"
    )]
    input: Option<PathBuf>,
    #[arg(long, help = "Kafka bootstrap servers to read a partition from")]
    brokers: Option<String>,
    #[arg(long, default_value = "dusk.ledger", help = "The ledger topic")]
    topic: String,
    #[arg(long, help = "The partition to read")]
    partition: Option<u32>,
    #[arg(long, help = "The first offset to read (default: the oldest retained)")]
    start: Option<i64>,
    #[arg(
        long,
        help = "The offset to stop before (default: the end of the partition)"
    )]
    end: Option<i64>,
    #[arg(
        long = "property",
        value_name = "NAME=VALUE",
        help = "A librdkafka property, repeatable"
    )]
    properties: Vec<String>,
    #[arg(long, default_value_t = 30, help = "Seconds to wait for Kafka")]
    timeout_seconds: u64,
}

#[derive(Args)]
struct LedgerHash {
    #[arg(
        long = "params",
        help = "A file holding the canonical Cap'n Proto encoding of the parameters"
    )]
    parameters: PathBuf,
    #[arg(
        long,
        help = "The param key file (default: ledger.param_key_file of the configuration)"
    )]
    key_file: Option<PathBuf>,
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("NIGHTFALL_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_current_span(false)
        .with_target(true)
        .init();
}

fn serve(path: &std::path::Path) -> anyhow::Result<ExitCode> {
    let config = Config::load(path)?;
    nightfall::admin::install_metrics();
    let broker = Arc::new(KafkaBroker::new(
        &config.kafka.brokers,
        &config.kafka.properties,
        &config.instance,
    ));
    let ledger_log = KafkaLog::connect(KafkaLogConfig {
        brokers: config.kafka.brokers.clone(),
        properties: config.kafka.properties.clone(),
        topic: config.kafka.topics.ledger.clone(),
        partition: config.ledger_partition()?,
        instance: config.instance.clone(),
        operation_timeout: Duration::from_secs(30),
    })
    .map_err(|error| anyhow::anyhow!("connect the ledger to Kafka: {error}"))?;
    let drain = Duration::from_secs(config.drain_seconds);
    let mut instance = nightfall::server::start(
        config,
        Services {
            broker,
            ledger_log: Box::new(ledger_log),
        },
    )?;
    let fenced = instance
        .handle()
        .block_on(nightfall::server::wait_for_stop(instance.shared.clone()));
    if !fenced {
        instance.drain(drain);
    }
    instance.shutdown();
    Ok(if fenced {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn verify(arguments: VerifyLedger) -> anyhow::Result<ExitCode> {
    let keys = VerifyingKeys::load(&arguments.keys)
        .map_err(|error| anyhow::anyhow!("{}: {error}", arguments.keys.display()))?;
    let options = VerifyOptions::default();
    let report = match (&arguments.input, &arguments.brokers) {
        (Some(input), None) => {
            if input.as_os_str() == "-" {
                verify_lines(std::io::stdin().lock(), keys, options)?
            } else {
                let file = std::fs::File::open(input)
                    .with_context(|| format!("open {}", input.display()))?;
                verify_lines(std::io::BufReader::new(file), keys, options)?
            }
        }
        (None, Some(brokers)) => {
            let partition = arguments
                .partition
                .context("--partition is required with --brokers")?;
            let mut properties = BTreeMap::new();
            for property in &arguments.properties {
                let (name, value) = property
                    .split_once('=')
                    .with_context(|| format!("--property {property:?} is not NAME=VALUE"))?;
                properties.insert(name.to_string(), value.to_string());
            }
            verify_partition(
                brokers,
                &properties,
                &PartitionRange {
                    topic: arguments.topic.clone(),
                    partition,
                    start: arguments.start,
                    end: arguments.end,
                },
                keys,
                options,
                Duration::from_secs(arguments.timeout_seconds),
            )
            .map_err(|error| anyhow::anyhow!("read the ledger partition: {error}"))?
        }
        _ => anyhow::bail!("give either --input or --brokers with --partition"),
    };
    print!("{report}");
    Ok(if report.is_clean() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn hash(config: &std::path::Path, arguments: LedgerHash) -> anyhow::Result<ExitCode> {
    let key_file = match arguments.key_file {
        Some(key_file) => key_file,
        None => Config::load(config)?.ledger.param_key_file,
    };
    let key = ParamKey::load(&key_file).map_err(|error| anyhow::anyhow!("{error}"))?;
    let parameters = std::fs::read(&arguments.parameters)
        .with_context(|| format!("read {}", arguments.parameters.display()))?;
    println!("{}", key.hash(&parameters));
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let command = Command::parse();
    let outcome = match command.action.unwrap_or(Action::Serve) {
        Action::Serve => {
            init_logging();
            serve(&command.config)
        }
        Action::VerifyLedger(arguments) => verify(arguments),
        Action::LedgerHash(arguments) => hash(&command.config, arguments),
    };
    match outcome {
        Ok(code) => code,
        Err(error) => {
            tracing::error!(error = format!("{error:#}"), "nightfall failed");
            eprintln!("nightfall: {error:#}");
            ExitCode::FAILURE
        }
    }
}
