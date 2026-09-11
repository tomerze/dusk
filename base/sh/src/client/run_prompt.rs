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
use tokio::sync::oneshot;

pub struct Created<S: ShEntriesBuilder> {
    pub client: dusk::Client,
    pub sh_entries_builder: S,
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
        let finished = self.finished.take();
        tokio::task::spawn_local(async move {
            if let Err(error) = prompt(client, sh_entries_builder, process).await {
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
) -> anyhow::Result<()> {
    let stop_signal = StopSignal::new();
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
        stop_signal.signal(),
    )
    .await?;
    let result = prompt.run().await;

    let mut kill_request = client.kill_request();
    kill_request.get().set_pid(shell.sh_pid);
    kill_request.get().set_signal(15);
    kill_request.send().promise.await?;
    result
}
