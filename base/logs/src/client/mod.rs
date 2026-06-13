use super::*;
use crate::logs_capnp::logs_args;
use capnp::capability::FromClientHook as _;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
use std::rc::Rc;

pub mod stream;

#[derive(clap::Parser)]
#[command(name = "logs", no_binary_name = true)]
struct LogsCli {
    #[arg(value_parser = ["view", "stream"])]
    command: Option<String>,
    /// The stream destination: file://, otlp://, http:// or https://.
    url: Option<String>,
    /// Only stream records at this severity or above.
    #[arg(short = 'l', long = "level", value_parser = ["error", "warn", "info", "debug", "trace"], default_value = "trace")]
    level: String,
    /// Replay the buffered history, then stop — a bounded snapshot that
    /// returns. Use this from scripts and MCP; a plain stream never returns.
    #[arg(long = "replay-only", conflicts_with = "follow_only")]
    replay_only: bool,
    /// Skip the buffered history; follow only records logged from now on.
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
    server: logs_args::server::Client,
}

impl LogsArgs {
    pub fn new(
        minimum_level: Option<log_record_capnp::SeverityNumber>,
        mode: logs_args::Mode,
        server: logs_args::server::Client,
    ) -> Self {
        let mut data = ArgsDataBuilder::new_default();
        {
            let mut root = data.init_root();
            if let Some(minimum_level) = minimum_level {
                root.set_level(minimum_level);
            }
            root.set_mode(mode);
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
        owned.with_root_builder(|root| {
            root.get_args()
                .init_server()
                .set_as_capability(self.server.into_client_hook());
            Ok(())
        })?;
        Ok(Rc::new(owned))
    }
}

struct LogsProgramArgsBuilder {}

impl ProgramArgsBuilder for LogsProgramArgsBuilder {
    fn build(&self, _client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = LogsCli::try_parse_from(args)?;
        let minimum_level = Some(severity_floor(&cli.level));
        // clap's `conflicts_with` already rejects both flags together.
        let mode = if cli.replay_only {
            logs_args::Mode::ReplayOnly
        } else if cli.follow_only {
            logs_args::Mode::FollowOnly
        } else {
            logs_args::Mode::ReplayThenFollow
        };
        let server = match cli.command.as_deref() {
            Some("stream") => {
                let url = cli
                    .url
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("`logs stream` needs a destination url"))?;
                stream::parse(url)?
            }
            _ => {
                if let Some(url) = &cli.url {
                    anyhow::bail!("`logs view` takes no url: {url}");
                } else if std::env::var_os("DUSK_NON_INTERACTIVE").is_some() {
                    // Set by gateways (the MCP server) that have no terminal to
                    // give away: the viewer would take over the process and hang
                    // a caller that can never press `q`.
                    anyhow::bail!(
                        "`logs view` is interactive and unavailable here. For a bounded \
                         snapshot that returns, use `logs --replay-only` (dumps the buffer \
                         to stdout) or `logs stream <url> --replay-only`."
                    );
                } else if cli.replay_only {
                    // A bounded snapshot with no terminal to take over: dump the
                    // buffered records to stdout, one per line, and exit.
                    capnp_rpc::new_client(stream::PrintStream::new())
                } else {
                    capnp_rpc::new_client(stream::ViewerStream::new())
                }
            }
        };
        Ok(LogsArgs::new(minimum_level, mode, server).as_program_args()?)
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
Use `logs` to view or stream the node's logs

Examples:
* `logs` or `logs view` opens an interactive log view.
Intended for use from an interactive context (The dusk prompt, python repl).
* `logs stream <url>` streams the logs to a URL.
* `logs stream <url> --replay-only` writes the buffered history to the URL and
EXITS — a bounded snapshot. Use this from scripts and MCP; a plain stream never
returns and pins its connection.

Modes:
(default)      replay the buffered history, then follow new records forever.
--replay-only  replay the buffered history, then stop (a bounded snapshot).
--follow-only  skip the history; follow only records logged from now on.

Supported url types:
file:// - streams in jsonl format to a file
otlp:// - streams in opentelemetry grpc to an otlp collector
http (or https):// - streams in HTTP(s) post requests where each POST is a JSON with a single log

Notes:
For a bounded read-and-return, prefer `--replay-only` and read the file.
For a live follow (no --replay-only), stream to a named pipe so it stays
bounded, run it as a background task, and `kill` it when done — a file follow
grows without limit.
If you are using MCP, put your connection descriptor in the file name
(e.g. `file:///tmp/dusk-logs-<descriptor>.jsonl`) so you won't collide with
other models.

`-l <level>` sets the minimum severity (error, warn, info, debug, trace).
The default is trace.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(LogsProgramArgsBuilder {}),
    }
}
