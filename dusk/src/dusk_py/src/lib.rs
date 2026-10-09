#![allow(exported_private_dependencies)]
#![feature(linkage)]

use connection::{COMMAND_QUEUE, Command, Options, Reply};
use dusk_connection::TlsClient;
use dusk_program::anyhow;
use dusk_program_sh::entry::{EntryInfo, sh_entries};
use dusk_program_sh::sh_capnp::DEFAULT_PID;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use shell_output::ShellOutput;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};

mod connection;
mod ctrl_c;
mod pool;
mod shell_output;

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

struct Link {
    commands: mpsc::Sender<Command>,
    shutdown: Mutex<Option<oneshot::Sender<Reply<()>>>>,
}

impl Link {
    fn send(&self, command: Command) -> PyResult<()> {
        if self
            .shutdown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none()
        {
            return Err(PyRuntimeError::new_err("Connection is closed"));
        }
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                TrySendError::Full(_) => PyRuntimeError::new_err(format!(
                    "{COMMAND_QUEUE} commands are already waiting on this connection, try again"
                )),
                TrySendError::Closed(_) => PyRuntimeError::new_err("Connection is closed"),
            })
    }
}

fn wait<Answer: Send>(
    py: Python<'_>,
    receiver: oneshot::Receiver<anyhow::Result<Answer>>,
) -> PyResult<Answer> {
    match py.detach(move || receiver.blocking_recv()) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(PyRuntimeError::new_err(format!("{error:#}"))),
        Err(_) => Err(PyRuntimeError::new_err("Connection is closed")),
    }
}

fn start(options: Options) -> PyResult<(Link, oneshot::Receiver<anyhow::Result<()>>)> {
    let (commands, receiver) = mpsc::channel(COMMAND_QUEUE);
    let (shutdown, shutdown_receiver) = oneshot::channel();
    let (ready, ready_receiver) = oneshot::channel();
    pool::pool()
        .and_then(|pool| {
            pool.spawn(move || connection::serve(options, receiver, shutdown_receiver, ready))
        })
        .map_err(PyRuntimeError::new_err)?;
    Ok((
        Link {
            commands,
            shutdown: Mutex::new(Some(shutdown)),
        },
        ready_receiver,
    ))
}

#[pyclass]
struct Dusk {
    link: Link,
}

#[pymethods]
impl Dusk {
    #[new]
    #[pyo3(signature = (address, port, sh_server_pid=None, *, server_name=None, ca=None, certificate=None, key=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python,
        address: String,
        port: u16,
        sh_server_pid: Option<u64>,
        server_name: Option<String>,
        ca: Option<PathBuf>,
        certificate: Option<PathBuf>,
        key: Option<PathBuf>,
    ) -> PyResult<Self> {
        let tls = match ca {
            Some(ca) => Some(TlsClient {
                server_name: server_name.unwrap_or_else(|| address.clone()),
                ca,
                certificate,
                key,
            }),
            None if server_name.is_some() || certificate.is_some() || key.is_some() => {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "server_name, certificate and key need ca, the trust anchors of a TLS connection",
                ));
            }
            None => None,
        };
        let (link, ready) = start(Options {
            address,
            port,
            sh_server_pid: sh_server_pid.unwrap_or(DEFAULT_PID),
            tls,
        })?;
        wait(py, ready)?;
        Ok(Dusk { link })
    }

    /// Disconnect from the Dusk server.
    /// This should be called when you're done using the client.
    fn disconnect(&mut self, py: Python) -> PyResult<()> {
        let shutdown = self
            .link
            .shutdown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("Already disconnected"))?;
        let (reply, receiver) = oneshot::channel();
        shutdown
            .send(reply)
            .map_err(|_| PyRuntimeError::new_err("Already disconnected"))?;
        wait(py, receiver)
    }

    /// Run `command` in this object's shell server, returning its output values.
    fn sh(&self, command: String) -> PyResult<ShellOutput> {
        let (output, shell_output) = ShellOutput::channel();
        self.link.send(Command::Sh { command, output })?;
        Ok(shell_output)
    }

    /// Open an interactive prompt on this object's shell server, on the calling
    /// terminal. Blocks while the prompt is up and returns when it is left.
    fn prompt(&mut self, py: Python) -> PyResult<()> {
        let (reply, receiver) = oneshot::channel();
        self.link.send(Command::Prompt { reply })?;
        wait(py, receiver)
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
        let programs: Vec<EntryInfo> = sh_entries().iter().map(|entry| entry.info).collect();

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
}
