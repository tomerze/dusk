use dusk_capnp::dusk_capnp::dusk::Client;
use dusk_capnp::dusk_capnp::stream;
use dusk_program::anyhow::Result;
use dusk_program::value::Value;
use pyo3::exceptions::PyStopAsyncIteration;
use pyo3::prelude::*;
use std::rc::Rc;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};

use crate::SHELL_OUTPUT_BUFFER_SIZE;

#[pyclass]
pub struct ShellOutput {
    rx: Arc<Mutex<mpsc::Receiver<Result<Vec<u8>>>>>,
}

impl ShellOutput {
    pub(crate) fn new(
        command: String,
        message_tx: &mpsc::UnboundedSender<crate::Message>,
    ) -> Result<Self, String> {
        let (output_tx, output_rx) = mpsc::channel(SHELL_OUTPUT_BUFFER_SIZE);
        message_tx
            .send(crate::Message::Sh(command, output_tx))
            .map_err(|e| format!("{:?}", e))?;

        Ok(Self {
            rx: Arc::new(Mutex::new(output_rx)),
        })
    }
}

#[pymethods]
impl ShellOutput {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python) -> Option<PyResult<Py<PyAny>>> {
        // Release the GIL while parking on the channel. A never-ending command
        // (a live `logs stream`) blocks here forever; holding the GIL across it
        // would freeze every other Python thread, so a concurrent command on the
        // same connection could never run.
        let rx = self.rx.clone();
        let received = py.detach(move || rx.blocking_lock().blocking_recv());
        match received {
            Some(Ok(pickle_bytes)) => Some(Python::attach(|py| unpickle(py, &pickle_bytes))),
            Some(Err(e)) => Some(Err(pyo3::exceptions::PyRuntimeError::new_err(
                e.to_string(),
            ))),
            None => None,
        }
    }

    /// Await the next value, without occupying a thread while waiting.
    ///
    /// `__next__` parks a whole OS thread on the channel for as long as the node
    /// stays quiet, so a caller reading many commands at once needs a thread for
    /// each. Awaiting this instead costs a task. The channel is `tokio`'s, whose
    /// `recv` needs no reactor of its own, so the future is driven by whatever
    /// event loop the caller is running — asyncio included.
    ///
    /// Raises `StopAsyncIteration` once the command has finished, so a caller can
    /// drive it exactly as they would an async iterator. It is not spelled
    /// `__anext__` because pyo3 fills that slot from a plain method rather than
    /// from an `async fn`; `ShellOutput` is the awaitable's owner either way.
    async fn next_value(&self) -> PyResult<Py<PyAny>> {
        let rx = self.rx.clone();
        let received = {
            let mut receiver = rx.lock().await;
            receiver.recv().await
        };
        match received {
            Some(Ok(pickle_bytes)) => Python::attach(|py| unpickle(py, &pickle_bytes)),
            Some(Err(e)) => Err(pyo3::exceptions::PyRuntimeError::new_err(e.to_string())),
            None => Err(PyStopAsyncIteration::new_err(())),
        }
    }
}

/// Turn one output value back into the Python object the node sent.
fn unpickle(py: Python<'_>, pickle_bytes: &[u8]) -> PyResult<Py<PyAny>> {
    let loads = pyo3::types::PyModule::import(py, "pickle")?.getattr("loads")?;
    Ok(loads.call1((pickle_bytes,))?.unbind())
}

pub struct StreamServer {
    pub(crate) tx: mpsc::Sender<Result<Vec<u8>>>,
    pub(crate) done_tx: Option<oneshot::Sender<()>>,
}

impl stream::Server for StreamServer {
    fn send(&mut self, params: stream::SendParams) -> capnp::capability::Promise<(), capnp::Error> {
        let tx = self.tx.clone();
        let result = params
            .get()
            .and_then(|p| p.get_value())
            .and_then(|value_reader| {
                Value::from_reader(value_reader).map_err(|e| capnp::Error::failed(e.to_string()))
            })
            .and_then(|value| {
                serde_pickle::to_vec(&value, Default::default())
                    .map_err(|e| capnp::Error::failed(e.to_string()))
            });

        capnp::capability::Promise::from_future(async move {
            if let Ok(pickle_bytes) = result {
                // This will block if the channel is full, providing backpressure
                let _ = tx.send(Ok(pickle_bytes)).await;
            }
            Ok(())
        })
    }

    fn done(
        &mut self,
        _: stream::DoneParams,
        _: stream::DoneResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        if let Some(tx) = self.done_tx.take() {
            let _ = tx.send(());
        }
        capnp::capability::Promise::ok(())
    }
}

pub fn handle_sh(client: Client, command: String, output_tx: mpsc::Sender<Result<Vec<u8>>>) {
    let (done_tx, done_rx) = oneshot::channel();
    let stream_server = StreamServer {
        tx: output_tx.clone(),
        done_tx: Some(done_tx),
    };
    let stream_client = capnp_rpc::new_client(stream_server);

    tokio::task::spawn_local(async move {
        // Create a new shell for this command
        let shell_result = dusk_shell::shell::Shell::new(
            client.clone(),
            dusk_program_sh::entry::StaticShEntriesBuilder::default(),
            dusk_program_sh::parser::Parser::new(),
        )
        .await;

        let mut shell = match shell_result {
            Ok(s) => s,
            Err(e) => {
                let _ = output_tx.send(Err(e)).await;
                return;
            }
        };

        // No external stop source in the Python embedding; a fresh Notify that
        // nothing ever fires keeps the server's stop_cap.stop() pending forever.
        let result = shell
            .sh(&command, stream_client, done_rx, Rc::new(Notify::new()))
            .await;

        // Kill the shell now that the command has completed
        let _ = shell.kill().await;

        if let Err(e) = result {
            let _ = output_tx.send(Err(e)).await;
        }
    });
}
