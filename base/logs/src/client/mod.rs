use super::*;
use crate::logs_capnp::logs_args;
use capnp::capability::FromClientHook as _;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
use std::rc::Rc;

pub mod convert;
pub mod stream;

#[derive(clap::Parser)]
#[command(name = "logs", no_binary_name = true)]
struct LogsCli {
    #[arg(value_parser = ["view", "stream", "dump"])]
    command: Option<String>,
    /// The stream destination: file://, otlp://, http:// or https://.
    url: Option<String>,
    /// Only stream signals at this severity or above.
    #[arg(short = 'l', long = "level", value_parser = ["error", "warn", "info", "debug", "trace"], default_value = "trace")]
    level: String,
    /// Replay the buffered history, then stop — a bounded snapshot that
    /// returns. Use this from scripts and MCP; a plain stream never returns.
    #[arg(long = "replay-only", conflicts_with = "follow_only")]
    replay_only: bool,
    /// Skip the buffered history; follow only signals logged from now on.
    #[arg(long = "follow-only")]
    follow_only: bool,
}

fn severity_floor(level: &str) -> log_record_capnp::SeverityNumber {
    match level {
        "error" => log_record_capnp::SeverityNumber::Error,
        "warn" => log_record_capnp::SeverityNumber::Warn,
        "info" => log_record_capnp::SeverityNumber::Info,
        "debug" => log_record_capnp::SeverityNumber::Debug,
        _ => log_record_capnp::SeverityNumber::Trace,
    }
}

pub struct LogsArgs {
    data: ArgsDataBuilder,
    server: Option<logs_args::server::Client>,
}

impl LogsArgs {
    pub fn new(
        minimum_level: Option<log_record_capnp::SeverityNumber>,
        flags: u8,
        server: Option<logs_args::server::Client>,
    ) -> Self {
        let mut data = ArgsDataBuilder::new_default();
        {
            let mut root = data.init_root();
            if let Some(minimum_level) = minimum_level {
                root.set_level(minimum_level);
            }
            root.set_flags(flags);
        }
        LogsArgs { data, server }
    }

    pub fn as_program_args(self) -> capnp::Result<Rc<ProgramArgs>> {
        let owned = ProgramArgs::new();
        owned.with_root_builder(|mut root| {
            root.set_program_id(PROGRAM_ID);
            let mut data_dest = root.init_args().init_data();
            let data_reader = self.data.get_root_as_reader()?;
            data_dest.set_as::<logs_capnp::logs_args::data::Owned>(data_reader)
        })?;
        if let Some(server) = self.server {
            owned.with_root_builder(|root| {
                root.get_args()
                    .init_server()
                    .set_as_capability(server.into_client_hook());
                Ok(())
            })?;
        }
        Ok(Rc::new(owned))
    }
}

struct LogsProgramArgsBuilder {}

#[dusk_program::async_trait::async_trait(?Send)]
impl ProgramArgsBuilder for LogsProgramArgsBuilder {
    async fn build(&self, client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = LogsCli::try_parse_from(args)?;
        let minimum_level = Some(severity_floor(&cli.level));
        let mut flags = if cli.replay_only {
            FLAG_REPLAY
        } else if cli.follow_only {
            FLAG_FOLLOW
        } else {
            FLAG_REPLAY | FLAG_FOLLOW
        };
        let server = match cli.command.as_deref() {
            Some("stream") => {
                let url = cli
                    .url
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("`logs stream` needs a destination url"))?;
                let namespace_id = client
                    .id_request()
                    .send()
                    .promise
                    .await?
                    .get()?
                    .get_result();
                Some(stream::parse(url, namespace_id)?)
            }
            Some("dump") => {
                if let Some(url) = &cli.url {
                    anyhow::bail!("`logs dump` takes no url: {url}");
                }
                flags |= FLAG_DUMP;
                None
            }
            _ => {
                if let Some(url) = &cli.url {
                    anyhow::bail!("`logs view` takes no url: {url}");
                }
                if cli.replay_only || cli.follow_only {
                    anyhow::bail!(
                        "`logs view` is an interactive view and has no bounded or \
                         follow-only mode. For logs that return as Dusk values, use \
                         `logs dump` (add --replay-only for a bounded snapshot, or \
                         --follow-only to skip the history)."
                    );
                }
                if std::env::var_os("DUSK_NON_INTERACTIVE").is_some() {
                    anyhow::bail!(
                        "`logs view` is interactive and unavailable here. For a bounded \
                         snapshot that returns, use `logs dump --replay-only`; to follow \
                         the live logs, use `logs dump`."
                    );
                }
                Some(capnp_rpc::new_client(stream::ViewerStream::new()))
            }
        };
        Ok(LogsArgs::new(minimum_level, flags, server).as_program_args()?)
    }
}

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(logs_capnp::PROGRAM_ID),
            name: "logs",
            short_description: "access logs",
            long_description: r#"
Use `logs` to read or stream the node's logs.

Subcommands:
* `logs` or `logs view` opens an interactive log pager.
Intended for an interactive context (the dusk prompt). It has no bounded or
follow-only mode; use `logs dump` when you need the logs as a command result.
* `logs dump` returns the logs as Dusk values — one record per signal — on the
command's own output stream. Use this from scripts, the python repl, and MCP:
its output comes back to the caller (unlike `view`, which paints a terminal, and
`stream`, which sends to an external sink).
* `logs stream <url>` streams the logs and tracing spans to an external URL.
* `logs stream <url> --replay-only` writes the buffered history to the URL and exits.

Modes (apply to `dump` and `stream`):
(default)      replay the buffered history, then follow new logged signals forever.
--replay-only  replay the buffered history, then stop (a bounded snapshot that returns).
--follow-only  skip the history; follow only signals logged from now on.

Examples:
* `logs dump --replay-only`         a bounded snapshot of the buffered logs.
* `logs dump`                       the snapshot, then follow live (until stopped).
* `logs dump --follow-only -l warn` follow new warnings and errors only.

Supported `stream` url types:
file:// - streams in jsonl format to a file
otlp:// - streams in opentelemetry grpc to an otlp collector
http (or https):// - streams in HTTP(s) post requests where each POST is a JSON with a single log

Notes:
* Tracing spans are logged too.
- When viewing logs via `logs view` spans don't show.
- `logs dump` emits spans as records alongside log records.
- when streaming to file:// or http(s):// they show as normal structured logs without severity.
- when streaming to otlp:// they show as otlp traces.

* `-l <level>` sets the minimum severity (error, warn, info, debug, trace).
The default is trace.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(LogsProgramArgsBuilder {}),
    }
}
