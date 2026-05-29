#![allow(exported_private_dependencies)]
#![feature(linkage)]

use dusk_capnp::dusk_capnp::dusk::Client;
use dusk_program::anyhow::Result;
use dusk_program_sh::entry::{EntryInfo, GetAvailableProgramsInfo, StaticShEntriesBuilder};
use dusk_shell::connection::Connection;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use tokio::sync::mpsc;

mod shell_output;
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
}

// Since we need the capnp rpc runtime to run using tokio on a single thread the design of this
// client is as follows:
// * When a Dusk instance is created, a new thread is spawned
// * This thread runs a tokio runtime with a local task set
// * The connection to the Dusk server is established in this thread
// * A mpsc channel is used to send messages to this thread for executing actions
// * The thread listens for messages and processes them accordingly
#[pyclass(unsendable)]
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
    fn new(_py: Python, address: String, port: u16) -> PyResult<Self> {
        let address = std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
            std::net::Ipv4Addr::from_str(&address)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?,
            port,
        ));

        let (message_tx, message_rx) = mpsc::unbounded_channel::<Message>();
        let (init_tx, mut init_rx) = mpsc::unbounded_channel::<Result<()>>();

        let thread_handle =
            std::thread::spawn(move || Self::connection_thread(address, message_rx, init_tx));

        // Wait for initialization to complete or fail
        match init_rx.blocking_recv() {
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
    fn disconnect(&mut self) -> PyResult<()> {
        self.disconnect_internal()
    }

    fn sh(&mut self, _py: Python, command: String) -> PyResult<ShellOutput> {
        let tx = self
            .message_tx
            .lock()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?
            .as_ref()
            .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("Connection is closed"))?
            .clone();

        ShellOutput::new(command, &tx).map_err(pyo3::exceptions::PyRuntimeError::new_err)
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
                    "no program named '{program_name}'"
                ))),
            }
        }
    }

    fn __del__(&mut self) {
        let _ = self.disconnect_internal();
    }
}

impl Dusk {
    fn connection_thread(
        address: std::net::SocketAddr,
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
                Ok((connection, client)) => {
                    let _ = init_tx.send(Ok(()));
                    (connection, client)
                }
                Err(e) => {
                    let _ = init_tx.send(Err(e));
                    return Ok(());
                }
            };

            loop {
                match message_rx.recv().await {
                    Some(Message::Sh(command, output_tx)) => {
                        handle_sh(client.clone(), command, output_tx);
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
    fn disconnect_internal(&mut self) -> PyResult<()> {
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
                let _ = handle.join();

                // Get the shutdown result
                if let Some(shutdown_result) = result_rx.blocking_recv() {
                    return shutdown_result
                        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()));
                }
            }
            Ok(())
        } else if let Some(handle) = handle_opt {
            // No sender but have handle - just wait for thread
            let _ = handle.join();
            Ok(())
        } else {
            // Already disconnected
            Err(pyo3::exceptions::PyRuntimeError::new_err(
                "Already disconnected",
            ))
        }
    }
}
