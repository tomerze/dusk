#![allow(exported_private_dependencies)]

use anyhow::Result;
use dusk_capnp::dusk_capnp::dusk::Client;
use dusk_shell::connection::Connection;
use pyo3::prelude::*;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use tokio::sync::mpsc;

mod shell_output;
use shell_output::{handle_sh, ShellOutput};

// Provide a dummy __pender symbol for embassy linkage compatibility
#[no_mangle]
static __pender: () = ();

/// Maximum number of items that can be buffered in the shell output channel.
/// This provides backpressure to prevent unbounded memory growth if the Python
/// consumer is slower than the Cap'n Proto stream producer.
pub(crate) const SHELL_OUTPUT_BUFFER_SIZE: usize = 32;

#[pymodule]
mod dusk {
    #[pymodule_export]
    use super::shell_output::ShellOutput;
    #[pymodule_export]
    use super::Dusk;
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
        let addr = std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
            std::net::Ipv4Addr::from_str(&address)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?,
            port,
        ));

        let (message_tx, message_rx) = mpsc::unbounded_channel::<Message>();
        let (init_tx, mut init_rx) = mpsc::unbounded_channel::<Result<()>>();

        let thread_handle =
            std::thread::spawn(move || Self::connection_thread(addr, message_rx, init_tx));

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

    fn __del__(&mut self) {
        let _ = self.disconnect_internal();
    }
}

impl Dusk {
    fn connection_thread(
        addr: std::net::SocketAddr,
        mut message_rx: mpsc::UnboundedReceiver<Message>,
        init_tx: mpsc::UnboundedSender<Result<()>>,
    ) -> Result<()> {
        let rt = tokio::runtime::Runtime::new()?;
        let local_set = tokio::task::LocalSet::new();

        async fn init(addr: std::net::SocketAddr) -> Result<(Connection, Client)> {
            let connection = Connection::connect(addr).await?;
            let client = connection.client().await;
            Ok((connection, client))
        }

        rt.block_on(local_set.run_until(async move {
            let (connection, client) = match init(addr).await {
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
            Ok::<(), anyhow::Error>(())
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
