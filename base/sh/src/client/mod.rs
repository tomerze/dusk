pub mod prompt;
pub mod shell;

use super::*;
use crate::ShMode;
use crate::entry::{
    EntryInfo, ProgramArgsBuilder, ShEntriesBuilder, ShEntry, StaticShEntriesBuilder,
};
use clap::Parser as _;
use dusk_capnp::dusk_capnp::{created, process};
use dusk_program::dusk_capnp::dusk_capnp::dusk;
use dusk_program::program_args::ProgramArgs;
use prompt::display_engine::DefaultDisplayEngine;
use prompt::stream::{display_stream, json_stream};
use prompt::{Prompt, StreamRequest};
use shell::Shell;
use std::rc::Rc;
use std::string::String;
use tokio::sync::{Notify, oneshot};

#[derive(clap::Parser)]
#[command(name = "sh", no_binary_name = true)]
struct ShCli {
    #[arg(short = 'd', long = "detach")]
    detach: bool,
    command: Option<String>,
}

pub struct Created<S: ShEntriesBuilder> {
    pub client: dusk::Client,
    pub sh_entries_builder: S,
    pub stop_signal: Rc<Notify>,
    pub finished: Option<oneshot::Sender<()>>,
}

impl<S: ShEntriesBuilder> created::Server for Created<S> {
    fn created(
        &mut self,
        params: created::CreatedParams,
        _results: created::CreatedResults,
    ) -> Promise<(), capnp::Error> {
        let process = pry!(pry!(params.get()).get_process());
        if std::env::var_os("DUSK_NON_INTERACTIVE").is_some() {
            return Promise::err(capnp::Error::failed(
                "there is no terminal to open a shell on: DUSK_NON_INTERACTIVE is set".to_string(),
            ));
        }
        let client = self.client.clone();
        let sh_entries_builder = self.sh_entries_builder.clone();
        let stop_signal = self.stop_signal.clone();
        let finished = self.finished.take();
        tokio::task::spawn_local(async move {
            if let Err(error) = prompt(client, sh_entries_builder, process, stop_signal).await {
                tracing::error!(error = %format!("{error:#}"), "the shell prompt failed");
            }
            if let Some(finished) = finished {
                let _ = finished.send(());
            }
        });
        Promise::ok(())
    }
}

async fn prompt<S: ShEntriesBuilder>(
    client: dusk::Client,
    sh_entries_builder: S,
    process: process::Client,
    stop_signal: Rc<Notify>,
) -> anyhow::Result<()> {
    let mut shell = Shell::new(
        client.clone(),
        sh_entries_builder.clone(),
        process.clone(),
        crate::parser::Parser::new(),
    )
    .await?;
    let stream_factory = |request: StreamRequest<DefaultDisplayEngine>| match request {
        StreamRequest::Raw => {
            let (json_stream, done_receiver) = json_stream::JsonStream::new_with_receiver(true);
            (capnp_rpc::new_client(json_stream), done_receiver)
        }
        StreamRequest::Display { display_engine } => {
            let (display_stream, done_receiver) =
                display_stream::DisplayStream::new_with_receiver(display_engine.clone());
            (capnp_rpc::new_client(display_stream), done_receiver)
        }
    };
    let prompt = Prompt::new(
        &mut shell,
        sh_entries_builder,
        DefaultDisplayEngine::default(),
        stream_factory,
        stop_signal,
    )
    .await?;
    let result = prompt.run().await;

    let mut kill_request = client.kill_request();
    kill_request.get().set_pid(shell.sh_pid);
    kill_request.get().set_signal(15);
    kill_request.send().promise.await?;
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
        Ok(ShArgs::new(client, StaticShEntriesBuilder::default(), mode)?.as_program_args()?)
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

* Use `sh <command>` (or `sh "<command>"`) to run a command.
* Use `sh -d <command>` to run it detached from the current session.
"#,
            version: VERSION,
        },
        program_args_builder: Rc::new(ShProgramArgsBuilder {}),
    }
}
