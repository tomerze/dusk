//! `http://` / `https://`: one POST per record, the record as a single
//! OTLP/JSON document.

use super::{records_json, stop_promise};
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

/// A `LogsArgs.Server` stream that POSTs each record as an OTLP/JSON document.
pub struct HttpStream {
    target: Result<Target, String>,
    stop: Rc<Notify>,
}

impl HttpStream {
    pub fn new(url: String) -> Self {
        let target = build_client()
            .map(|client| Target { client, url })
            .map_err(|error| format!("{error:#}"));
        HttpStream {
            target,
            stop: Rc::new(Notify::new()),
        }
    }
}

impl logs_args::server::Server for HttpStream {
    fn send(&mut self, params: logs_args::server::SendParams) -> Promise<(), capnp::Error> {
        let entries = dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_entries());
        let batch = records_json(entries);
        let stop = self.stop.clone();
        let (client, url) = match &self.target {
            Ok(target) => (target.client.clone(), target.url.clone()),
            Err(error) => {
                tracing::error!(error = %error, "the http log stream is unusable");
                stop.notify_one();
                return Promise::ok(());
            }
        };
        Promise::from_future(async move {
            if let Err(error) = post(&client, &url, batch).await {
                tracing::error!(error = %format!("{error:#}"), "the http log stream failed");
                stop.notify_one();
            }
            Ok(())
        })
    }

    fn stop(
        &mut self,
        _params: logs_args::server::StopParams,
        _results: logs_args::server::StopResults,
    ) -> Promise<(), capnp::Error> {
        stop_promise(&self.stop)
    }
}

/// POST each record as a single OTLP/JSON document. Awaiting the posts is the
/// backpressure — a slow endpoint parks the node's stream.
async fn post(client: &reqwest::Client, url: &str, batch: Vec<serde_json::Value>) -> Result<()> {
    for record in batch {
        client
            .post(url)
            .json(&record)
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .with_context(|| format!("posting to {url}"))?;
    }
    Ok(())
}

/// The reqwest client. With `DUSK_CLIENT_SKIP_TLS_VERIFY` set it skips certificate
/// validation so an `https://` target can be a self-signed collector — set
/// only by tests; unset (always, in production) it validates normally.
fn build_client() -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder();
    if std::env::var_os("DUSK_CLIENT_SKIP_TLS_VERIFY").is_some() {
        builder = builder.danger_accept_invalid_certs(true);
    }
    builder.build().context("building the http client")
}
