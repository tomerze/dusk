use super::*;
use crate::logs_capnp::logs_args;
use crate::stream;
use capnp::capability::FromClientHook as _;
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh::entry::{EntryInfo, ProgramArgsBuilder, ShEntry};
use std::rc::Rc;

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
        server: logs_args::server::Client,
    ) -> Self {
        let mut data = ArgsDataBuilder::new_default();
        {
            let mut root = data.init_root();
            if let Some(minimum_level) = minimum_level {
                root.set_level(minimum_level);
            }
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
                }
                // Set by gateways (the MCP server) that have no terminal to
                // give away: the viewer would take over the process and hang
                // a caller that can never press `q`.
                if std::env::var_os("DUSK_NON_INTERACTIVE").is_some() {
                    anyhow::bail!(
                        "`logs view` is interactive and unavailable here; \
                         stream to a file instead: `logs stream file:///tmp/dusk-logs.jsonl`. \
                         If you are using MCP, put your connection descriptor in the file \
                         name (e.g. `file:///tmp/dusk-logs-<descriptor>.jsonl`) so you won't \
                         collide with other models."
                    );
                }
                capnp_rpc::new_client(stream::ViewerStream::new())
            }
        };
        Ok(LogsArgs::new(minimum_level, server).as_program_args()?)
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
Not designed for scripts or MCP (these would rather stream to a file).
* `logs stream <url>` streams the logs to a URL.

Supported url types:
file:// - streams in jsonl format to a file
otlp:// - streams in opentelemetry grpc to an otlp collector
http (or https):// - streams in HTTP(s) post requests where each POST is a JSON with a single log

Notes:
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
