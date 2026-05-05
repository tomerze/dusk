use dusk_capnp::dusk_capnp::dusk::Client;
use dusk_capnp::dusk_capnp::stream;
use dusk_program::anyhow::Result;
use dusk_program::value::Value;
use pyo3::prelude::*;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

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
        let mut rx = self.rx.lock().ok()?;
        match rx.blocking_recv() {
            Some(Ok(pickle_bytes)) => {
                // Deserialize pickle bytes to Python object
                match pyo3::types::PyModule::import(py, "pickle") {
                    Ok(pickle_mod) => match pickle_mod.getattr("loads") {
                        Ok(loads) => match loads.call1((pickle_bytes.as_slice(),)) {
                            Ok(obj) => Some(Ok(obj.unbind())),
                            Err(e) => Some(Err(e)),
                        },
                        Err(e) => Some(Err(e)),
                    },
                    Err(e) => Some(Err(e)),
                }
            }
            Some(Err(e)) => Some(Err(pyo3::exceptions::PyRuntimeError::new_err(
                e.to_string(),
            ))),
            None => None,
        }
    }
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

        // No external stop source in the Python embedding; let the command run to completion.
        let result = shell
            .sh(&command, stream_client, done_rx, std::future::pending())
            .await;

        // Kill the shell now that the command has completed
        let _ = shell.kill().await;

        if let Err(e) = result {
            let _ = output_tx.send(Err(e)).await;
        }
    });
}
