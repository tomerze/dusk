use crate::client::args::program_args_for_command;
use crate::client::prompt::display_engine::DefaultDisplayEngine;
use crate::client::prompt::stream::{display_stream, json_stream};
use crate::client::prompt::{Prompt, StreamRequest};
use crate::client::shell::Shell;
use crate::client::stop::StopSignal;
use crate::entry::ShEntriesBuilder;
use crate::{ShArgs, ShMode};
use capnp::capability::Promise;
use dusk_capnp::capnp_rpc;
use dusk_capnp::dusk_capnp::{created, dusk, process};
use dusk_capnp::pry;
use dusk_program::anyhow;
use std::cell::Cell;
use std::format;
use std::rc::Rc;
use std::string::String;
use std::thread_local;
use tokio::sync::Notify;

const TERMINATE: u64 = 15;
const RECONNECT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

thread_local! {
    static PROMPT_OPEN: Cell<bool> = const { Cell::new(false) };
    static PROMPT_DONE: Rc<Notify> = Rc::new(Notify::new());
}

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
        PROMPT_OPEN.set(true);
        tokio::task::spawn_local(async move {
            if let Err(error) = prompt(client.clone(), sh_entries_builder, server_pid).await {
                tracing::error!(error = %format!("{error:#}"), "the prompt failed");
            }
            PROMPT_OPEN.set(false);
            PROMPT_DONE.with(|done| done.notify_waiters());
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
    let connecting_stop = StopSignal::new();
    let mut view_opened = false;
    loop {
        let program_args = if view_opened {
            let mut sh_args = ShArgs::new(
                client.clone(),
                sh_entries_builder.clone(),
                ShMode::Prompt {
                    client_hostname: client_hostname(),
                    server_pid,
                },
            )?;
            sh_args.created = None;
            sh_args.as_program_args()?
        } else {
            program_args_for_command(
                client.clone(),
                sh_entries_builder.clone(),
                &format!("sh --prompt {server_pid}"),
            )
            .await?
        };
        let attempt = async {
            let mut process_request = client.process_request();
            program_args.with_reader(|reader| process_request.get().set_program_args(reader))?;
            let view = process_request.send().promise.await?.get()?.get_result()?;
            view_opened = true;
            let pid = view.pid_request().send().promise.await?.get()?.get_result();
            let done = PROMPT_DONE.with(|done| done.clone());
            let ending_client = client.clone();
            let ending_view = view.clone();
            let ending = tokio::task::spawn_local(async move {
                done.notified().await;
                if let Err(error) = kill(&ending_client, &ending_view, TERMINATE).await {
                    tracing::debug!(error = %error, "the prompt's process was already gone");
                }
            });
            let run_reply = view.run_request().send().promise.await;
            ending.abort();
            run_reply?;
            let mut waitpid_request = client.waitpid_request();
            waitpid_request.get().set_pid(pid);
            waitpid_request.send().promise.await?;
            Ok::<(), capnp::Error>(())
        }
        .await;
        match attempt {
            Ok(()) => return Ok(()),
            Err(error)
                if error.kind == capnp::ErrorKind::Disconnected
                    && view_opened
                    && !PROMPT_OPEN.get() =>
            {
                return Ok(());
            }
            Err(error) if error.kind == capnp::ErrorKind::Disconnected => {
                let stop = connecting_stop.signal();
                match dusk_program::futures::future::select(
                    core::pin::pin!(tokio::time::sleep(RECONNECT_INTERVAL)),
                    core::pin::pin!(stop.notified()),
                )
                .await
                {
                    dusk_program::futures::future::Either::Left(_) => {}
                    dusk_program::futures::future::Either::Right(_) => return Ok(()),
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
}
