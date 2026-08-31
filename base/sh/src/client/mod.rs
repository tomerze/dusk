pub mod prompt;
pub mod shell;
pub mod terminal;

use super::*;
use crate::ShMode;
use crate::entry::{EntryInfo, ProgramArgsBuilder, ShEntry, StaticShEntriesBuilder};
use clap::Parser as _;
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use std::rc::Rc;

#[derive(clap::Parser)]
#[command(name = "sh", no_binary_name = true)]
struct ShCli {
    #[arg(short = 'd', long = "detach")]
    detach: bool,
    #[arg(long = "new", conflicts_with = "command")]
    new: bool,
    command: Option<String>,
}

struct NestedPrompt {
    client: dusk::Client,
}

impl dusk_program::dusk_capnp::dusk_capnp::created::Server for NestedPrompt {
    fn created(
        &mut self,
        params: dusk_program::dusk_capnp::dusk_capnp::created::CreatedParams,
        _results: dusk_program::dusk_capnp::dusk_capnp::created::CreatedResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        let process = pry!(pry!(params.get()).get_process());
        let Some(reading) = terminal::try_read() else {
            tracing::info!("the terminal is taken, the new shell gets no prompt");
            return capnp::capability::Promise::ok(());
        };
        let client = self.client.clone();
        tokio::task::spawn_local(async move {
            if let Err(error) = nested_prompt(client, process, reading).await {
                tracing::error!("nested prompt failed: {error:?}");
            }
        });
        capnp::capability::Promise::ok(())
    }
}

async fn nested_prompt(
    client: dusk::Client,
    process: dusk_program::dusk_capnp::dusk_capnp::process::Client,
    reading: terminal::ReadingGuard,
) -> anyhow::Result<()> {
    let compiler = capnp_rpc::new_client(crate::ShCompiler {
        client: client.clone(),
        sh_entries_builder: StaticShEntriesBuilder::default(),
    });
    let mut shell =
        shell::Shell::new(client, process, compiler, crate::parser::Parser::new()).await?;
    let stream_factory = |request: prompt::StreamRequest<
        prompt::display_engine::DefaultDisplayEngine,
    >| match request {
        prompt::StreamRequest::Raw => {
            let (json_stream, done_receiver) =
                prompt::stream::json_stream::JsonStream::new_with_receiver(true);
            (capnp_rpc::new_client(json_stream), done_receiver)
        }
        prompt::StreamRequest::Display { display_engine } => {
            let (display_stream, done_receiver) =
                prompt::stream::display_stream::DisplayStream::new_with_receiver(
                    display_engine.clone(),
                );
            (capnp_rpc::new_client(display_stream), done_receiver)
        }
    };
    let result = async {
        prompt::Prompt::new(
            &mut shell,
            StaticShEntriesBuilder::default(),
            prompt::display_engine::DefaultDisplayEngine::default(),
            stream_factory,
            Rc::new(tokio::sync::Notify::new()),
        )
        .await?
        .run(reading)
        .await
    }
    .await;
    shell.detach();
    result
}

struct ShProgramArgsBuilder {}

#[dusk_program::async_trait::async_trait(?Send)]
impl ProgramArgsBuilder for ShProgramArgsBuilder {
    async fn build(&self, client: dusk::Client, args: &[&str]) -> anyhow::Result<Rc<ProgramArgs>> {
        let cli = ShCli::try_parse_from(args)?;
        let mode = match cli.command {
            None => ShMode::Server,
            Some(command) if cli.detach => ShMode::DetachedScript(command),
            Some(command) => ShMode::Script(command),
        };
        let is_server = matches!(mode, ShMode::Server);
        let program_args = ShArgs::new(client.clone(), StaticShEntriesBuilder::default(), mode)?
            .as_program_args()?;
        if is_server {
            if cli.new {
                program_args.set_created(capnp_rpc::new_client(NestedPrompt { client }))?;
            } else {
                program_args.set_pid(Some(sh_capnp::SERVER_PID))?;
            }
        }
        Ok(program_args)
    }
}

#[dusk_program_sh_proc::sh_entry]
pub fn sh_entry() -> ShEntry {
    ShEntry {
        info: EntryInfo {
            program_id: Some(sh_capnp::PROGRAM_ID),
            name: "sh",
            short_description: "run Dusk shell commands",
            long_description: r#"
`sh` runs commands in the Dusk shell — Dusk's own shell language, not a Unix
shell. Dusk shell commands run Dusk programs built into the Dusk Node.

* Use `sh` on its own to attach to the node's shell.
* Use `sh <command>` (or `sh "<command>"`) to run a command.
* Use `sh -d <command>` to run it detached from the current session.
* Use `sh --new` to start a shell of its own rather than attach to the node's.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(ShProgramArgsBuilder {}),
    }
}
