//! `file://<path>`: appends each streamed signal as one OTLP/JSON line (jsonl).

use super::acknowledge;
use crate::client::convert::signal_to_json;
use crate::logs_capnp::logs_args;
use capnp::capability::Promise;
use dusk_program::anyhow::{Context as _, Result};
use std::path::PathBuf;
use std::rc::Rc;
use std::string::String;
use std::vec::Vec;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::{Mutex, Notify};

/// The file, opened lazily on the first batch so construction stays sync. A
/// failed open or write drops back to [`State::Unopened`] so the next attempt
/// reopens.
enum State {
    Unopened(PathBuf),
    Open {
        file: tokio::fs::File,
        path: PathBuf,
    },
}

/// A `LogsArgs.Server` stream that appends OTLP/JSON signals, one per line, to
/// a file.
pub struct FileStream {
    state: Rc<Mutex<State>>,
    stop: Rc<Notify>,
    namespace_id: u64,
}

impl FileStream {
    pub fn new(file_path: PathBuf, namespace_id: u64) -> Self {
        FileStream {
            state: Rc::new(Mutex::new(State::Unopened(file_path))),
            stop: Rc::new(Notify::new()),
            namespace_id,
        }
    }
}

impl Drop for FileStream {
    fn drop(&mut self) {
        tracing::info!("the file log stream closed");
    }
}

impl logs_args::server::Server for FileStream {
    fn send(&mut self, params: logs_args::server::SendParams) -> Promise<(), capnp::Error> {
        let signal_batch = dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_signal_batch());
        let entries = dusk_capnp::pry!(signal_batch.get_signals());
        let ack = dusk_capnp::pry!(signal_batch.get_ack());
        // Render each signal to its OTLP/JSON line; a span is a structured log to a
        // file consumer. A signal that fails to convert is skipped with a warning.
        let mut batch = Vec::new();
        for entry in entries {
            match signal_to_json(entry, self.namespace_id) {
                Ok(value) => batch.push(value),
                Err(error) => tracing::warn!(%error, "skipping an unconvertible signal"),
            }
        }
        let logs_count = entries.len();
        let state = self.state.clone();
        Promise::from_future(async move {
            // A failed write goes unacknowledged so the node re-sends it; the
            // stream stays up rather than failing on a transient write error.
            if let Err(error) = append(&state, &batch).await {
                let message = format!("{error:#}");
                tracing::warn!(logs_count, error = %message, "failed streaming over file");
                return Ok(());
            }
            acknowledge(ack).await;
            tracing::info!(logs_count, "streaming over file...");
            Ok(())
        })
    }

    fn stop(
        &mut self,
        _params: logs_args::server::StopParams,
        _results: logs_args::server::StopResults,
    ) -> Promise<(), capnp::Error> {
        let stop = self.stop.clone();
        Promise::from_future(async move {
            stop.notified().await;
            Ok(())
        })
    }
}

/// Open on first use, then append `batch` as jsonl. Awaiting the write is the
/// backpressure — a slow file/pipe parks the node's stream.
async fn append(state: &Rc<Mutex<State>>, batch: &[serde_json::Value]) -> Result<()> {
    let mut guard = state.lock().await;
    if let State::Unopened(file_path) = &*guard {
        let file_path = file_path.clone();
        // A failed open leaves the state Unopened, so the retry reopens.
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .await
            .with_context(|| format!("opening {}", file_path.display()))?;
        *guard = State::Open {
            file,
            path: file_path,
        };
    }
    let State::Open { file, path } = &mut *guard else {
        return Ok(());
    };
    let mut lines = String::new();
    for signal in batch {
        lines.push_str(&serde_json::to_string(signal)?);
        lines.push('\n');
    }
    if let Err(error) = file.write_all(lines.as_bytes()).await {
        // Reopen on the next attempt: the handle may be the casualty.
        let path = path.clone();
        *guard = State::Unopened(path);
        return Err(error).context("writing to the file log stream");
    }
    Ok(())
}
