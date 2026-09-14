//! `http://` / `https://`: one POST per signal, the signal as a single
//! OTLP/JSON document.

use super::acknowledge;
use crate::client::convert::signal_to_json;
use crate::logs_capnp::logs_args;
use capnp::capability::Promise;
use dusk_program::anyhow::{Context as _, Result};
use std::rc::Rc;
use std::string::String;
use std::vec::Vec;
use tokio::sync::Notify;

/// The POST target plus its client, built once. A builder failure is held as a
/// message and surfaced on the first batch, keeping construction sync.
struct Target {
    client: reqwest::Client,
    url: String,
}

/// A `LogsArgs.Stream` that POSTs each signal as an OTLP/JSON document.
pub struct HttpStream {
    target: Result<Target, String>,
    stop: Rc<Notify>,
    namespace_id: u64,
}

impl HttpStream {
    pub fn new(url: String, namespace_id: u64) -> Self {
        let target = build_client()
            .map(|client| Target { client, url })
            .map_err(|error| format!("{error:#}"));
        HttpStream {
            target,
            stop: Rc::new(Notify::new()),
            namespace_id,
        }
    }
}

impl Drop for HttpStream {
    fn drop(&mut self) {
        tracing::info!("the http log stream closed");
    }
}

impl logs_args::stream::Server for HttpStream {
    fn send(&mut self, params: logs_args::stream::SendParams) -> Promise<(), capnp::Error> {
        let signal_batch = dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_signal_batch());
        let entries = dusk_capnp::pry!(signal_batch.get_signals());
        let ack = dusk_capnp::pry!(signal_batch.get_ack());
        // Render each signal to its OTLP/JSON document; a span is a structured log
        // to an http consumer. A signal that fails to convert is skipped.
        let mut batch = Vec::new();
        for entry in entries {
            match signal_to_json(entry, self.namespace_id) {
                Ok(value) => batch.push(value),
                Err(error) => tracing::warn!(%error, "skipping an unconvertible signal"),
            }
        }
        let logs_count = entries.len();
        let stop = self.stop.clone();
        let (client, url) = match &self.target {
            Ok(target) => (target.client.clone(), target.url.clone()),
            Err(error) => {
                // A construction failure is not a connection failure; there is
                // nothing to retry.
                tracing::error!(error = %error, "the http log stream is unusable");
                stop.notify_one();
                return Promise::ok(());
            }
        };
        Promise::from_future(async move {
            if let Err(error) = post(&client, &url, &batch).await {
                let message = format!("{error:#}");
                tracing::warn!(logs_count, error = %message, "failed streaming over http");
                return Ok(());
            }
            acknowledge(ack).await;
            tracing::info!(logs_count, "streaming over http...");
            Ok(())
        })
    }

    fn stop(
        &mut self,
        _params: logs_args::stream::StopParams,
        _results: logs_args::stream::StopResults,
    ) -> Promise<(), capnp::Error> {
        let stop = self.stop.clone();
        Promise::from_future(async move {
            stop.notified().await;
            Ok(())
        })
    }
}

/// POST each signal as a single OTLP/JSON document. Awaiting the posts is the
/// backpressure - a slow endpoint parks the node's stream.
async fn post(client: &reqwest::Client, url: &str, batch: &[serde_json::Value]) -> Result<()> {
    for signal in batch {
        client
            .post(url)
            .json(signal)
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .with_context(|| format!("posting to {url}"))?;
    }
    Ok(())
}

/// The reqwest client. With `DUSK_CLIENT_SKIP_TLS_VERIFY` set it skips certificate
/// validation so an `https://` target can be a self-signed collector - set
/// only by tests; unset (always, in production) it validates normally.
fn build_client() -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder();
    if std::env::var_os("DUSK_CLIENT_SKIP_TLS_VERIFY").is_some() {
        builder = builder.tls_danger_accept_invalid_certs(true);
    }
    builder.build().context("building the http client")
}
