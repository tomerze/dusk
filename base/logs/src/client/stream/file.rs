//! `file://<path>`: appends each streamed record as one OTLP/JSON line (jsonl).

use super::{records_json, stop_promise};
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
/// fatal failure latches [`State::Failed`] so later batches drop rather than
/// retry a dead file.
enum State {
    Unopened(PathBuf),
    Open(tokio::fs::File),
    Failed,
}

/// A `LogsArgs.Server` stream that appends OTLP/JSON records, one per line, to
/// a file.
pub struct FileStream {
    state: Rc<Mutex<State>>,
    stop: Rc<Notify>,
}

impl FileStream {
    pub fn new(file_path: PathBuf) -> Self {
        FileStream {
            state: Rc::new(Mutex::new(State::Unopened(file_path))),
            stop: Rc::new(Notify::new()),
        }
    }
}

impl logs_args::server::Server for FileStream {
    fn send(&mut self, params: logs_args::server::SendParams) -> Promise<(), capnp::Error> {
        let entries = dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_entries());
        let batch = records_json(entries);
        let state = self.state.clone();
        let stop = self.stop.clone();
        Promise::from_future(async move {
            if let Err(error) = append(&state, batch).await {
                tracing::error!(error = %format!("{error:#}"), "the file log stream failed");
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

/// Open on first use, then append `batch` as jsonl. Awaiting the write is the
/// backpressure — a slow file/pipe parks the node's stream.
async fn append(state: &Rc<Mutex<State>>, batch: Vec<serde_json::Value>) -> Result<()> {
    let mut guard = state.lock().await;
    if let State::Unopened(file_path) = &*guard {
        let file_path = file_path.clone();
        match tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .await
        {
            Ok(file) => *guard = State::Open(file),
            Err(error) => {
                *guard = State::Failed;
                return Err(error).with_context(|| format!("opening {}", file_path.display()));
            }
        }
    }
    let State::Open(file) = &mut *guard else {
        return Ok(());
    };
    let mut lines = String::new();
    for record in batch {
        lines.push_str(&serde_json::to_string(&record)?);
        lines.push('\n');
    }
    file.write_all(lines.as_bytes())
        .await
        .context("writing to the file log stream")?;
    Ok(())
}
