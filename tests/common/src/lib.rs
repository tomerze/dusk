use capnp::capability::Promise;
use dusk_base::dusk_program_init::Args as InitArgs;
use dusk_connection::Connection;
use dusk_program_logs::client::LogsArgs;
use dusk_program_logs::common_capnp::any_value;
use dusk_program_logs::log_record_capnp::{SeverityNumber, log_record};
use dusk_program_logs::{FLAG_FOLLOW, FLAG_REPLAY, logs_args, signal};
use rand::Rng;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const LISTEN_ADDRESS: &str = "127.0.0.1";

pub fn gen_port() -> u16 {
    let mut rng = rand::rng();
    rng.random_range(1001..=65535)
}

/// A `LogsArgs.Stream` that records the body of every ERROR-severity entry the
/// node streams to it - the harness's error monitor, hosted directly rather
/// than going through `logs stream <url>`.
struct ErrorCaptureStream {
    errors: Arc<Mutex<Vec<String>>>,
}

impl logs_args::stream::Server for ErrorCaptureStream {
    fn send(&mut self, params: logs_args::stream::SendParams) -> Promise<(), capnp::Error> {
        let signal_batch = match params.get().and_then(|params| params.get_signal_batch()) {
            Ok(signal_batch) => signal_batch,
            Err(error) => return Promise::err(error),
        };
        let entries = match signal_batch.get_signals() {
            Ok(entries) => entries,
            Err(error) => return Promise::err(error),
        };
        let ack = match signal_batch.get_ack() {
            Ok(ack) => ack,
            Err(error) => return Promise::err(error),
        };
        {
            let mut errors = self.errors.lock().unwrap();
            for entry in entries.iter() {
                if matches!(entry.get_severity_number(), Ok(SeverityNumber::Error))
                    && let Ok(signal::Which::LogRecord(Ok(log_record))) = entry.which()
                {
                    errors.push(body_text(log_record));
                }
            }
        }
        // Acknowledge so the node frees the batch instead of re-sending it.
        Promise::from_future(async move {
            if let Err(error) = ack.ack_request().send().promise.await {
                eprintln!("error monitor failed to acknowledge a batch: {error}");
            }
            Ok(())
        })
    }

    fn stop(
        &mut self,
        _params: logs_args::stream::StopParams,
        _results: logs_args::stream::StopResults,
    ) -> Promise<(), capnp::Error> {
        // Long-poll: the monitor streams for the node's whole lifetime.
        Promise::from_future(std::future::pending())
    }
}

/// An entry's body as text, for the failure message; empty when it has none.
fn body_text(entry: log_record::Reader) -> String {
    entry
        .get_body()
        .ok()
        .and_then(|body| body.which().ok())
        .and_then(|which| match which {
            any_value::Which::StringValue(Ok(text)) => text.to_str().ok().map(str::to_string),
            _ => None,
        })
        .unwrap_or_default()
}
pub struct DuskNixImpl {
    errors: Arc<Mutex<Vec<String>>>,
    errors_expected: AtomicBool,
}

impl DuskNixImpl {
    pub fn new(address: &str, port: u16) -> Self {
        let address = address.to_string();

        let node_address = address.clone();
        std::thread::spawn(move || {
            let init_script =
                dusk_program_sh::compile::compile(&format!("nightfall -l {node_address}:{port}"))
                    .expect("compile the init script");
            let init_program_args = InitArgs::new(&init_script)
                .expect("build init args")
                .as_program_args()
                .expect("build init program_args");
            let launcher_set =
                dusk_base::default_launcher_set().expect("build the base launcher set");
            dusk_nix::run(move || Ok(launcher_set.clone()), init_program_args);
        });

        // Block until the server is accepting connections.
        let socket_address = format!("{address}:{port}");
        loop {
            if std::net::TcpStream::connect(&socket_address).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        let errors = Arc::new(Mutex::new(Vec::new()));

        // Monitor: a `logs` process streaming to our own ErrorCaptureStream for
        // the node's lifetime.
        let monitor_errors = errors.clone();
        let monitor_address = address.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build the monitor runtime");
            let local = tokio::task::LocalSet::new();
            local.block_on(&runtime, async move {
                let address: SocketAddr = format!("{monitor_address}:{port}")
                    .parse()
                    .expect("monitor address");
                let Ok(connection) = Connection::connect(address).await else {
                    return;
                };
                let client = connection.client().await;
                let Ok(program_args) = LogsArgs::new(None, FLAG_REPLAY | FLAG_FOLLOW, move || {
                    Ok(capnp_rpc::new_client(ErrorCaptureStream {
                        errors: monitor_errors.clone(),
                    }))
                })
                .as_program_args() else {
                    return;
                };

                let mut process_request = client.process_request();
                if program_args
                    .with_reader(|reader| process_request.get().set_program_args(reader))
                    .is_err()
                {
                    return;
                }
                let process = match process_request.send().promise.await {
                    Ok(reply) => match reply.get().and_then(|result| result.get_result()) {
                        Ok(process) => process,
                        Err(_) => return,
                    },
                    Err(_) => return,
                };
                let mut run_request = client.run_request();
                run_request.get().set_process(process);
                if run_request.send().promise.await.is_err() {
                    return;
                }

                // Keep the connection (and the hosted ErrorCaptureStream) alive
                // for the node's lifetime so the daemon keeps streaming to it.
                std::future::pending::<()>().await;
            });
        });

        Self {
            errors,
            errors_expected: AtomicBool::new(false),
        }
    }

    /// Fail if any ERROR-severity signal reached the monitor. Gives the
    /// asynchronous stream a moment to drain first, so an error logged just
    /// before this call is not missed.
    pub fn assert_no_errors(&self) {
        if self.errors_expected.load(Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(Duration::from_millis(300));
        let errors = self.errors.lock().unwrap();
        if !errors.is_empty() {
            let mut message = String::from("errors were logged by dusk:\n");
            for line in errors.iter() {
                message.push_str(&format!("  {line}\n"));
            }
            panic!("{message}");
        }
    }

    pub fn expect_errors(&self) {
        self.errors_expected.store(true, Ordering::SeqCst);
    }
}

impl Drop for DuskNixImpl {
    fn drop(&mut self) {
        // A failing test is already unwinding; panicking again here would abort
        // the process (taking sibling tests with it) and bury the original
        // failure. Let the in-progress panic carry the real message.
        if std::thread::panicking() {
            return;
        }
        self.assert_no_errors();
    }
}
