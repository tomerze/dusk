#![allow(exported_private_dependencies)]
#![feature(linkage)]

use dusk_capnp::dusk_capnp::dusk::Client;
use dusk_connection::Connection;
use dusk_program::anyhow::Result;
use dusk_program_sh::client::open_prompt;
use dusk_program_sh::client::shell::Shell;
use dusk_program_sh::entry::{EntryInfo, GetAvailableProgramsInfo, StaticShEntriesBuilder};
use dusk_program_sh::sh_capnp::DEFAULT_PID;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use std::rc::Rc;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use tokio::sync::Mutex as TokioMutex;
use tokio::sync::mpsc;

mod ctrl_c;
mod shell_output;
use ctrl_c::stop_on_ctrl_c;
use shell_output::{ShellOutput, handle_sh};

// Provide a dummy __pender symbol for embassy linkage compatibility
#[unsafe(no_mangle)]
#[linkage = "weak"]
static __pender: () = ();

/// Maximum number of items that can be buffered in the shell output channel.
/// This provides backpressure to prevent unbounded memory growth if the Python
/// consumer is slower than the Cap'n Proto stream producer.
pub(crate) const SHELL_OUTPUT_BUFFER_SIZE: usize = 32;

/// Register the dusk Python module.
/// This function is called by the wrapper binary in bins/dusk_py.
pub fn register_module(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<Dusk>()?;
    module.add_class::<shell_output::ShellOutput>()?;
    Ok(())
}

/// Build a Python dict describing one available program.
///
/// Keys: `name`, `version`, `short_description`, `long_description`.
fn entry_info_to_dict<'py>(py: Python<'py>, info: &EntryInfo) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("name", info.name)?;
    dict.set_item("version", info.version)?;
    dict.set_item("short_description", info.short_description)?;
    dict.set_item("long_description", info.long_description)?;
    dict.set_item("program_id", info.program_id)?;
    Ok(dict)
}

pub(crate) enum Message {
    Shutdown(mpsc::UnboundedSender<Result<()>>),
    Sh(String, mpsc::Sender<Result<Vec<u8>>>),
    Prompt(mpsc::UnboundedSender<Result<()>>),
}

// Since we need the capnp rpc runtime to run using tokio on a single thread the design of this
// client is as follows:
// * When a Dusk instance is created, a new thread is spawned
// * This thread runs a tokio runtime with a local task set
// * The connection to the Dusk server is established in this thread
// * A mpsc channel is used to send messages to this thread for executing actions
// * The thread listens for messages and processes them accordingly
#[pyclass]
struct Dusk {
    thread_handle: Arc<Mutex<Option<JoinHandle<Result<()>>>>>,
    message_tx: Arc<Mutex<Option<mpsc::UnboundedSender<Message>>>>,
}

#[pymethods]
impl Dusk {
    /// Create a new Dusk client and connect to the server.
    ///
    /// Args:
    ///     address: The IP address
    ///     port: The port number
    ///
    /// Returns:
    ///     A Dusk client instance
    #[new]
    #[pyo3(signature = (address, port, sh_server_pid=None))]
    fn new(py: Python, address: String, port: u16, sh_server_pid: Option<u64>) -> PyResult<Self> {
        let address = std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
            std::net::Ipv4Addr::from_str(&address)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?,
            port,
        ));

        let (message_tx, message_rx) = mpsc::unbounded_channel::<Message>();
        let (init_tx, mut init_rx) = mpsc::unbounded_channel::<Result<()>>();

        let server_pid = sh_server_pid.unwrap_or(DEFAULT_PID);
        let thread_handle = std::thread::spawn(move || {
            Self::connection_thread(address, server_pid, message_rx, init_tx)
        });

        // Wait for initialization to complete or fail, releasing the GIL so a
        // slow connect doesn't freeze other Python threads.
        match py.detach(|| init_rx.blocking_recv()) {
            Some(Ok(())) => Ok(Dusk {
                thread_handle: Arc::new(Mutex::new(Some(thread_handle))),
                message_tx: Arc::new(Mutex::new(Some(message_tx))),
            }),
            Some(Err(e)) => Err(pyo3::exceptions::PyRuntimeError::new_err(e.to_string())),
            None => Err(pyo3::exceptions::PyRuntimeError::new_err(
                "Initialization failed unexpectedly",
            )),
        }
    }

    /// Disconnect from the Dusk server.
    /// This should be called when you're done using the client.
    fn disconnect(&mut self, py: Python) -> PyResult<()> {
        self.disconnect_internal(py)
    }

    /// Run `command` in this object's shell server, returning its output values.
    fn sh(&self, command: String) -> PyResult<ShellOutput> {
        let sender = self.sender()?;
        ShellOutput::new(command, &sender).map_err(pyo3::exceptions::PyRuntimeError::new_err)
    }

    /// Open an interactive prompt on this object's shell server, on the calling
    /// terminal. Blocks while the prompt is up and returns when it is left.
    fn prompt(&mut self, py: Python) -> PyResult<()> {
        let sender = self.sender()?;
        let (result_sender, mut result_receiver) = mpsc::unbounded_channel();
        sender
            .send(Message::Prompt(result_sender))
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(format!("{error:?}")))?;
        match py.detach(move || result_receiver.blocking_recv()) {
            Some(Ok(())) => Ok(()),
            Some(Err(error)) => Err(pyo3::exceptions::PyRuntimeError::new_err(format!(
                "{error:#}"
            ))),
            None => Err(pyo3::exceptions::PyRuntimeError::new_err(
                "the prompt ended without a result",
            )),
        }
    }

    /// Look up help for the available programs.
    ///
    /// With no `program_name` (or an empty string), returns a list of dicts,
    /// one per available program. With a program name, returns a single dict
    /// for that program, or raises `ValueError` if no such program exists.
    ///
    /// Each dict has keys `name`, `version`, `short_description`,
    /// `long_description`, and `program_id` (an int, or `None` for builtins).
    ///
    /// The set of available programs is determined at link time on the client
    /// side, so this is a static method and does not require a connection.
    #[staticmethod]
    #[pyo3(signature = (program_name = String::new()))]
    fn help(py: Python<'_>, program_name: String) -> PyResult<Bound<'_, PyAny>> {
        let programs = StaticShEntriesBuilder::default()
            .get_available_programs_info()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        if program_name.is_empty() {
            let dicts = programs
                .iter()
                .map(|info| entry_info_to_dict(py, info))
                .collect::<PyResult<Vec<_>>>()?;
            Ok(PyList::new(py, dicts)?.into_any())
        } else {
            match programs.iter().find(|info| info.name == program_name) {
                Some(info) => Ok(entry_info_to_dict(py, info)?.into_any()),
                None => Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "no sh entry named '{program_name}'"
                ))),
            }
        }
    }

    fn __del__(&mut self, py: Python) {
        let _ = self.disconnect_internal(py);
    }
}

impl Dusk {
    fn sender(&self) -> PyResult<mpsc::UnboundedSender<Message>> {
        Ok(self
            .message_tx
            .lock()
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?
            .as_ref()
            .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("Connection is closed"))?
            .clone())
    }

    fn connection_thread(
        address: std::net::SocketAddr,
        server_pid: u64,
        mut message_rx: mpsc::UnboundedReceiver<Message>,
        init_tx: mpsc::UnboundedSender<Result<()>>,
    ) -> Result<()> {
        let rt = tokio::runtime::Runtime::new()?;
        let local_set = tokio::task::LocalSet::new();

        async fn init(address: std::net::SocketAddr) -> Result<(Connection, Client)> {
            let connection = Connection::connect(address).await?;
            let client = connection.client().await;
            Ok((connection, client))
        }

        rt.block_on(local_set.run_until(async move {
            let (connection, client) = match init(address).await {
                Ok(pair) => pair,
                Err(e) => {
                    let _ = init_tx.send(Err(e));
                    return Ok(());
                }
            };

            // The capnp client connects lazily, so `init` returns Ok even
            // against a server that is down - the refused socket only surfaces
            // on the first real RPC. Force one round-trip here so connection
            // setup fails up front, instead of the first command the caller
            // runs after `Dusk(...)` appears to succeed.
            if let Err(e) = client.hostname_request().send().promise.await {
                let _ = connection.disconnect().await;
                let _ = init_tx.send(Err(e.into()));
                return Ok(());
            }
            let shell = async {
                let process = Shell::recreate_sh_process(
                    client.clone(),
                    StaticShEntriesBuilder::default(),
                    server_pid,
                )
                .await?;
                Shell::new(
                    client.clone(),
                    StaticShEntriesBuilder::default(),
                    process,
                )
                .await
            }
            .await;
            let shell = match shell {
                Ok(shell) => Rc::new(TokioMutex::new(shell)),
                Err(error) => {
                    if let Err(disconnect_error) = connection.disconnect().await {
                        tracing::warn!(error = %format!("{disconnect_error:#}"), "couldn't disconnect after the shell server failed to start");
                    }
                    if init_tx.send(Err(error)).is_err() {
                        tracing::warn!("nobody was waiting for the connection to be set up");
                    }
                    return Ok(());
                }
            };
            let _ = init_tx.send(Ok(()));

            loop {
                match message_rx.recv().await {
                    Some(Message::Sh(command, output_tx)) => {
                        handle_sh(shell.clone(), command, output_tx);
                    }
                    Some(Message::Prompt(result_sender)) => {
                        let client = client.clone();
                        tokio::task::spawn_local(async move {
                            let result = stop_on_ctrl_c(open_prompt(
                                client,
                                StaticShEntriesBuilder::default(),
                                server_pid,
                            ))
                            .await;
                            if let Err(error) = result_sender.send(result) {
                                tracing::warn!(error = %format!("{error:?}"), "nobody was waiting for the prompt's result");
                            }
                        });
                    }
                    Some(Message::Shutdown(result_tx)) => {
                        Self::handle_shutdown(client, connection, result_tx).await;
                        break;
                    }
                    None => {
                        drop(client);
                        let _ = connection.disconnect().await;
                        break;
                    }
                }
            }
            Ok::<(), dusk_program::anyhow::Error>(())
        }))
    }
    async fn handle_shutdown(
        client: Client,
        connection: Connection,
        result_tx: mpsc::UnboundedSender<Result<()>>,
    ) {
        let result = async {
            drop(client);
            connection.disconnect().await?;
            Ok(())
        }
        .await;
        let _ = result_tx.send(result);
    }

    // Disconnection flow
    // * User calls `disconnect()` which wraps this internal function
    // * A shutdown message is sent to the connection thread
    // * The thread calls `connection.disconnect()`
    fn disconnect_internal(&mut self, py: Python) -> PyResult<()> {
        // Take the sender if available
        let tx_opt = self
            .message_tx
            .lock()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?
            .take();

        // Take the handle if available
        let handle_opt = self
            .thread_handle
            .lock()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?
            .take();

        // If we have a sender, send shutdown and wait for result
        if let Some(tx) = tx_opt {
            let (result_tx, mut result_rx) = mpsc::unbounded_channel();
            let _ = tx.send(Message::Shutdown(result_tx));

            // Wait for thread to finish if we have a handle
            if let Some(handle) = handle_opt {
                // Release the GIL while joining the connection thread and
                // waiting for its shutdown result - both block.
                let shutdown_result = py.detach(move || {
                    let _ = handle.join();
                    result_rx.blocking_recv()
                });
                if let Some(shutdown_result) = shutdown_result {
                    return shutdown_result
                        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()));
                }
            }
            Ok(())
        } else if let Some(handle) = handle_opt {
            // No sender but have handle - just wait for the thread, GIL released.
            py.detach(move || {
                let _ = handle.join();
            });
            Ok(())
        } else {
            // Already disconnected
            Err(pyo3::exceptions::PyRuntimeError::new_err(
                "Already disconnected",
            ))
        }
    }
}
