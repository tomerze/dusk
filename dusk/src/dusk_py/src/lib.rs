use anyhow::Result;
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
    ///     address: The IP address or hostname as a string
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

        let (message_tx, mut message_rx) = mpsc::unbounded_channel::<Message>();
        let (init_tx, mut init_rx) = mpsc::unbounded_channel::<Result<()>>();

        let thread_handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new()?;
            let local_set = tokio::task::LocalSet::new();

            rt.block_on(local_set.run_until(async move {
                let init_result = async {
                    let connection = dusk_shell::connection::Connection::connect(addr).await?;
                    let client = connection.client().await;
                    let shell =
                        dusk_shell::shell::Shell::new(dusk_program_sh::engine::ShEngine::new(
                            client,
                            dusk_program_sh::entry::StaticShEntriesBuilder::default(),
                        ))
                        .await?;
                    Ok::<_, anyhow::Error>((connection, shell))
                }
                .await;

                match init_result {
                    Ok((connection, shell)) => {
                        let _ = init_tx.send(Ok(()));

                        // Wait for shutdown message or channel closure
                        match message_rx.recv().await {
                            Some(Message::Shutdown(result_tx)) => {
                                let result = async {
                                    shell.kill().await?;
                                    connection.disconnect().await?;
                                    Ok(())
                                }
                                .await;
                                let _ = result_tx.send(result);
                            }
                            None => {
                                // Channel closed, clean up anyway
                                let _ = shell.kill().await;
                                let _ = connection.disconnect().await;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = init_tx.send(Err(e));
                    }
                }
                Ok::<(), anyhow::Error>(())
            }))
        });

        // Wait for initialization to complete or fail
        match init_rx.blocking_recv() {
            Some(Ok(())) => Ok(Dusk {
                thread_handle: Arc::new(Mutex::new(Some(thread_handle))),
                message_tx: Arc::new(Mutex::new(Some(message_tx))),
            }),
            Some(Err(e)) => Err(pyo3::exceptions::PyRuntimeError::new_err(e.to_string())),
            None => Err(pyo3::exceptions::PyRuntimeError::new_err(
                "Initialization failed",
            )),
        }
    }

    /// Disconnect from the Dusk server.
    /// This should be called when you're done using the client.
    fn disconnect(&self) -> PyResult<()> {
        self.disconnect_impl()
    }

    fn __del__(&mut self) {
        let _ = self.disconnect_impl();
    }
}

impl Dusk {
    fn disconnect_impl(&self) -> PyResult<()> {
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
