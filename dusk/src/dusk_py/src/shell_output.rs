use dusk_program::anyhow::Result;
use dusk_program::stream::{Stream, StreamMixin};
use dusk_program::value::Value;
use dusk_program_sh::client::shell::Shell;
use dusk_program_sh::client::stop::StopSignal;
use pyo3::exceptions::{PyRuntimeError, PyStopAsyncIteration};
use pyo3::prelude::*;
use pyo3::types::PyList;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

use crate::SHELL_OUTPUT_BUFFER_SIZE;

struct Output {
    rx: mpsc::Receiver<Result<Vec<u8>>>,
    shown: VecDeque<Result<Vec<u8>>>,
}

#[pyclass]
pub struct ShellOutput {
    output: Arc<Mutex<Output>>,
}

impl ShellOutput {
    fn value_at(&self, py: Python, index: usize) -> Option<std::result::Result<Vec<u8>, String>> {
        let output = self.output.clone();
        py.detach(move || {
            let mut output = output.blocking_lock();
            if index >= output.shown.len() {
                let value = output.rx.blocking_recv()?;
                output.shown.push_back(value);
            }
            match output.shown.get(index)? {
                Ok(pickle_bytes) => Some(Ok(pickle_bytes.clone())),
                Err(error) => Some(Err(format!("{error:#}"))),
            }
        })
    }

    pub(crate) fn new(
        command: String,
        message_tx: &mpsc::UnboundedSender<crate::Message>,
    ) -> Result<Self, String> {
        let (output_tx, output_rx) = mpsc::channel(SHELL_OUTPUT_BUFFER_SIZE);
        message_tx
            .send(crate::Message::Sh(command, output_tx))
            .map_err(|e| format!("{:?}", e))?;

        Ok(Self {
            output: Arc::new(Mutex::new(Output {
                rx: output_rx,
                shown: VecDeque::new(),
            })),
        })
    }
}

#[pymethods]
impl ShellOutput {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&self, py: Python) -> Option<PyResult<Py<PyAny>>> {
        // Release the GIL while parking on the channel. A never-ending command
        // (a live `logs stream`) blocks here forever; holding the GIL across it
        // would freeze every other Python thread, so a concurrent command on the
        // same connection could never run.
        let output = self.output.clone();
        let received = py.detach(move || {
            let mut output = output.blocking_lock();
            match output.shown.pop_front() {
                Some(value) => Some(value),
                None => output.rx.blocking_recv(),
            }
        });
        match received {
            Some(Ok(pickle_bytes)) => Some(Python::attach(|py| unpickle(py, &pickle_bytes))),
            Some(Err(e)) => Some(Err(PyRuntimeError::new_err(format!("{e:#}")))),
            None => None,
        }
    }

    /// Await the next value, without occupying a thread while waiting.
    ///
    /// `__next__` parks a whole OS thread on the channel for as long as the node
    /// stays quiet, so a caller reading many commands at once needs a thread for
    /// each. Awaiting this instead costs a task. The channel is `tokio`'s, whose
    /// `recv` needs no reactor of its own, so the future is driven by whatever
    /// event loop the caller is running - asyncio included.
    ///
    /// Raises `StopAsyncIteration` once the command has finished, so a caller can
    /// drive it exactly as they would an async iterator. It is not spelled
    /// `__anext__` because pyo3 fills that slot from a plain method rather than
    /// from an `async fn`; `ShellOutput` is the awaitable's owner either way.
    async fn next_value(&self) -> PyResult<Py<PyAny>> {
        let output = self.output.clone();
        let received = {
            let mut output = output.lock().await;
            match output.shown.pop_front() {
                Some(value) => Some(value),
                None => output.rx.recv().await,
            }
        };
        match received {
            Some(Ok(pickle_bytes)) => Python::attach(|py| unpickle(py, &pickle_bytes)),
            Some(Err(e)) => Err(PyRuntimeError::new_err(format!("{e:#}"))),
            None => Err(PyStopAsyncIteration::new_err(())),
        }
    }

    fn __repr__(&self, py: Python) -> PyResult<String> {
        let values = PyList::empty(py);
        let mut index = 0;
        while let Some(value) = self.value_at(py, index) {
            let pickle_bytes = value.map_err(PyRuntimeError::new_err)?;
            values.append(unpickle(py, &pickle_bytes)?)?;
            index += 1;
        }
        Ok(values.repr()?.to_str()?.to_string())
    }

    fn _ipython_display_(&self, py: Python) -> PyResult<()> {
        let mut index = 0;
        while let Some(value) = self.value_at(py, index) {
            let pickle_bytes = value.map_err(PyRuntimeError::new_err)?;
            let rendered = unpickle(py, &pickle_bytes)?;
            let line = format!("{}\n", rendered.bind(py).repr()?.to_str()?);
            let stdout = py.import("sys")?.getattr("stdout")?;
            stdout.call_method1("write", (line,))?;
            stdout.call_method0("flush")?;
            index += 1;
        }
        Ok(())
    }
}

/// Turn one output value back into the Python object the node sent.
fn unpickle(py: Python<'_>, pickle_bytes: &[u8]) -> PyResult<Py<PyAny>> {
    let loads = pyo3::types::PyModule::import(py, "pickle")?.getattr("loads")?;
    Ok(loads.call1((pickle_bytes,))?.unbind())
}

pub struct StreamServer {
    pub(crate) tx: mpsc::Sender<Result<Vec<u8>>>,
    pub(crate) done: Option<tokio::sync::oneshot::Sender<()>>,
}

impl StreamMixin for StreamServer {
    fn send(&mut self, value: Value) -> capnp::capability::Promise<(), capnp::Error> {
        let tx = self.tx.clone();
        let result = serde_pickle::to_vec(&value, Default::default())
            .map_err(|e| capnp::Error::failed(e.to_string()));

        capnp::capability::Promise::from_future(async move {
            if let Ok(pickle_bytes) = result {
                // This will block if the channel is full, providing backpressure
                let _ = tx.send(Ok(pickle_bytes)).await;
            }
            Ok(())
        })
    }

    fn end(&mut self) {
        if let Some(done) = self.done.take()
            && done.send(()).is_err()
        {
            tracing::debug!("nobody was waiting for the command to finish");
        }
    }
}

pub fn handle_sh(
    shell: Rc<Mutex<Shell>>,
    command: String,
    output_tx: mpsc::Sender<Result<Vec<u8>>>,
) {
    let (done_sender, done_receiver) = tokio::sync::oneshot::channel();
    let stream_server = Stream::new(StreamServer {
        tx: output_tx.clone(),
        done: Some(done_sender),
    });
    let stream_client = capnp_rpc::new_client(stream_server);

    tokio::task::spawn_local(async move {
        let stop_signal = StopSignal::new();
        let command_run =
            shell
                .lock()
                .await
                .sh(&command, stream_client, done_receiver, stop_signal.signal());
        let result = match command_run {
            Ok(command_run) => command_run.await,
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            let message = format!("{error:#}");
            if output_tx.send(Err(error)).await.is_err() {
                tracing::warn!(error = %message, "nobody was reading the command's error");
            }
        }
    });
}
