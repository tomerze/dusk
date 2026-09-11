use crate::client::args::program_args_for_command;
use crate::client::prompt::display_engine::DefaultDisplayEngine;
use crate::client::prompt::stream::{display_stream, json_stream};
use crate::client::prompt::{Prompt, StreamRequest};
use crate::client::shell::Shell;
use crate::client::stop::StopSignal;
use crate::entry::ShEntriesBuilder;
use capnp::capability::Promise;
use dusk_capnp::capnp_rpc;
use dusk_capnp::dusk_capnp::{created, dusk, process};
use dusk_capnp::pry;
use dusk_program::anyhow;
use std::format;
use std::string::String;

const TERMINATE: u64 = 15;

pub struct Created<S: ShEntriesBuilder> {
    pub client: dusk::Client,
    pub sh_entries_builder: S,
    pub server_pid: u64,
}

pub fn client_hostname() -> String {
    if let Some(overridden) = std::env::var_os("DUSK_CLIENT_HOSTNAME") {
        return overridden.to_string_lossy().into_owned();
    }
    match nix::unistd::gethostname() {
        Ok(hostname) => hostname.to_string_lossy().into_owned(),
        Err(error) => {
            tracing::warn!(error = %error, "couldn't read this machine's hostname");
            String::from("unknown")
        }
    }
}

impl<S: ShEntriesBuilder> created::Server for Created<S> {
    fn created(
        &mut self,
        params: created::CreatedParams,
        _results: created::CreatedResults,
    ) -> Promise<(), capnp::Error> {
        let view = pry!(pry!(params.get()).get_process());
        if std::env::var_os("DUSK_NON_INTERACTIVE").is_some() {
            return Promise::err(capnp::Error::failed(
                "there is no terminal to open a prompt on: DUSK_NON_INTERACTIVE is set".to_string(),
            ));
        }
        let client = self.client.clone();
        let sh_entries_builder = self.sh_entries_builder.clone();
        let server_pid = self.server_pid;
        tokio::task::spawn_local(async move {
            if let Err(error) = prompt(client.clone(), sh_entries_builder, server_pid).await {
                tracing::error!(error = %format!("{error:#}"), "the prompt failed");
            }
            if let Err(error) = kill(&client, &view, TERMINATE).await {
                tracing::debug!(error = %error, "the prompt's process was already gone");
            }
        });
        Promise::ok(())
    }
}

async fn kill(client: &dusk::Client, process: &process::Client, signal: u64) -> capnp::Result<()> {
    let pid = process
        .pid_request()
        .send()
        .promise
        .await?
        .get()?
        .get_result();
    let mut kill_request = client.kill_request();
    kill_request.get().set_pid(pid);
    kill_request.get().set_signal(signal);
    kill_request.send().promise.await?;
    Ok(())
}

async fn prompt<S: ShEntriesBuilder>(
    client: dusk::Client,
    sh_entries_builder: S,
    server_pid: u64,
) -> anyhow::Result<()> {
    let stop_signal = StopSignal::new();
    let server =
        Shell::recreate_sh_process(client.clone(), sh_entries_builder.clone(), server_pid).await?;
    let mut shell = Shell::new(
        client.clone(),
        sh_entries_builder.clone(),
        server,
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
        stop_signal.signal(),
    )
    .await?;
    prompt.run().await
}

pub async fn open_prompt<S: ShEntriesBuilder>(
    client: dusk::Client,
    sh_entries_builder: S,
    server_pid: u64,
) -> anyhow::Result<()> {
    let program_args = program_args_for_command(
        client.clone(),
        sh_entries_builder,
        &format!("sh --prompt {server_pid}"),
    )
    .await?;
    let mut process_request = client.process_request();
    program_args.with_reader(|reader| process_request.get().set_program_args(reader))?;
    let view = process_request.send().promise.await?.get()?.get_result()?;
    let pid = view.pid_request().send().promise.await?.get()?.get_result();
    view.run_request().send().promise.await?;
    let mut waitpid_request = client.waitpid_request();
    waitpid_request.get().set_pid(pid);
    waitpid_request.send().promise.await?;
    Ok(())
}
