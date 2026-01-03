use anyhow::Result;
use dusk_capnp::dusk_capnp::dusk::Client;
use dusk_shell::connection::Connection;
use pyo3::prelude::*;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use tokio::sync::mpsc;

// Provide a dummy __pender symbol for embassy linkage compatibility
#[no_mangle]
static __pender: () = ();

#[pymodule]
mod dusk {
    #[pymodule_export]
    use super::Dusk;
}

enum Message {
    Shutdown(mpsc::UnboundedSender<Result<()>>),
}

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

    fn sh(&mut self, _py: Python, _command: String) -> PyResult<()> {
        Ok(())
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
            // let shell =
            //     dusk_shell::shell::Shell::new(dusk_program_sh::engine::ShEngine::new(
            //         client,
            //         dusk_program_sh::entry::StaticShEntriesBuilder::default(),
            //     ))
            //     .await?;
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

            // Wait for shutdown message or channel closure
            // Keep the client alive here for future use
            match message_rx.recv().await {
                Some(Message::Shutdown(result_tx)) => {
                    // Drop client before disconnecting
                    drop(client);
                    let result = async {
                        connection.disconnect().await?;
                        Ok(())
                    }
                    .await;
                    let _ = result_tx.send(result);
                }
                None => {
                    // Channel closed, clean up anyway
                    drop(client);
                    connection.disconnect().await?;
                }
            }
            Ok::<(), anyhow::Error>(())
        }))
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
