//! End-to-end tests for the node's log streams: drive an in-process node and
//! assert its log records reach each stream - an HTTP collector, an HTTPS
//! collector (self-signed), and an OTLP/gRPC collector (all via the
//! `logs stream <url>` shell path), plus a custom in-memory `LogsArgs.Stream`
//! an external author could write (driven straight through the SDK, since a
//! custom stream has no url).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::routing::post;

use capnp::capability::FromClientHook as _;
use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::stream;
use dusk_connection::Connection;
use dusk_program::anyhow;
use dusk_program::stream::{Stream, StreamMixin};
use dusk_program_logs::client::LogsArgs;
use dusk_program_logs::common_capnp::any_value;
use dusk_program_logs::{FLAG_FOLLOW, FLAG_REPLAY, logs_args, signal};
use dusk_program_sh::sh_capnp;
use dusk_program_sh::{ShArgs, ShMode};
use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS, gen_port};

use opentelemetry_proto::tonic::collector::logs::v1::logs_service_server::{
    LogsService, LogsServiceServer,
};
use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use opentelemetry_proto::tonic::logs::v1::LogRecord as OtlpLogRecord;

use tokio::task::LocalSet;

/// How long to wait for a record to reach the destination before failing.
const ARRIVAL_TIMEOUT: Duration = Duration::from_secs(15);

const MARKER: &str = "logs stream opened";

// ---- driving `logs stream` ----

/// The sh result stream. `logs stream` never pushes command output here (it
/// streams records to the args server instead), so both methods are no-ops -
/// it only exists because `Shell::sh` requires an output stream.
struct OutputSink;

impl StreamMixin for OutputSink {
    fn send(&mut self, _value: dusk_program::value::Value) -> Promise<(), capnp::Error> {
        Promise::ok(())
    }

    fn end(&mut self) {}
}

/// Poll `predicate` until it holds or `timeout` elapses; returns whether it
/// held.
async fn wait_until(mut predicate: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if predicate() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn drive_logs_stream(
    port: u16,
    command: &str,
    predicate: Box<dyn FnMut() -> bool>,
) -> anyhow::Result<bool> {
    let address: SocketAddr = format!("{LISTEN_ADDRESS}:{port}").parse().unwrap();
    let connection = Connection::connect(address).await?;
    let client = connection.client().await;

    // `logs stream` runs until torn down, so its done long-poll never fires and
    // nothing ever asks it to stop; we cancel it by dropping the `sh` future.
    let output: stream::Client = capnp_rpc::new_client(Stream::new(OutputSink));
    let script = dusk_program_sh::compile(client.clone(), command).await?;
    let program_args = ShArgs::new(ShMode::Script(script))?.as_program_args()?;
    let drive = async {
        let mut process_request = client.process_request();
        program_args.with_reader(|reader| process_request.get().set_program_args(reader))?;
        let process_reply = process_request.send().promise.await?;
        let process = process_reply.get()?.get_result()?;
        let mut run_request = client.run_request();
        run_request.get().set_process(process.clone());
        run_request.send().promise.await?;
        let portal_reply = process.portal_request().send().promise.await?;
        let portal = portal_reply
            .get()?
            .get_result()?
            .cast_to::<sh_capnp::output_portal::Client>();
        let mut output_request = portal.output_request();
        output_request.get().set_stream(output);
        output_request.send().promise.await?;
        Ok::<(), capnp::Error>(())
    };

    let found = tokio::select! {
        result = drive => {
            // `logs stream` should outlive the wait; if it returned, surface why.
            result?;
            false
        }
        found = wait_until(predicate, ARRIVAL_TIMEOUT) => found,
    };

    let _ = connection.disconnect().await;
    Ok(found)
}

// ---- receivers ----

async fn http_handler(
    State(received): State<Arc<Mutex<Vec<serde_json::Value>>>>,
    Json(record): Json<serde_json::Value>,
) -> StatusCode {
    received.lock().unwrap().push(record);
    StatusCode::OK
}

/// An HTTP collector that records each POSTed OTLP/JSON body. Returns the
/// `http://…` URL and the shared sink.
async fn spawn_http_collector() -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/", post(http_handler))
        .with_state(received.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}/"), received)
}

/// Only aws-lc-rs is linked - reqwest's `rustls` feature, axum-server's
/// `tls-rustls` and rustls' own default all select it - so rustls 0.23 resolves the
/// process-wide default itself. Installing it explicitly keeps these tests working
/// if a dependency ever links `ring` as well, which turns that resolution into a
/// panic. Installing once is enough; a later attempt returns `Err` and is ignored.
fn install_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// An HTTPS collector with a self-signed certificate. The streaming client
/// trusts it via the `DUSK_CLIENT_SKIP_TLS_VERIFY' env variable.
async fn spawn_https_collector() -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/", post(http_handler))
        .with_state(received.clone());

    let certificate =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string(), "localhost".to_string()])
            .unwrap();
    let config = axum_server::tls_rustls::RustlsConfig::from_pem(
        certificate.cert.pem().into_bytes(),
        certificate.key_pair.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, config)
            .serve(app.into_make_service())
            .await
            .unwrap();
    });
    (format!("https://{address}/"), received)
}

/// An OTLP/gRPC log collector. Records every exported log record.
struct GrpcCollector {
    received: Arc<Mutex<Vec<OtlpLogRecord>>>,
}

#[tonic::async_trait]
impl LogsService for GrpcCollector {
    async fn export(
        &self,
        request: tonic::Request<ExportLogsServiceRequest>,
    ) -> Result<tonic::Response<ExportLogsServiceResponse>, tonic::Status> {
        let mut received = self.received.lock().unwrap();
        for resource_logs in request.into_inner().resource_logs {
            for scope_logs in resource_logs.scope_logs {
                received.extend(scope_logs.log_records);
            }
        }
        Ok(tonic::Response::new(ExportLogsServiceResponse {
            partial_success: None,
        }))
    }
}

/// Stand up the gRPC collector; returns the `otlp://…` URL and the shared sink.
async fn spawn_grpc_collector() -> (String, Arc<Mutex<Vec<OtlpLogRecord>>>) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let collector = GrpcCollector {
        received: received.clone(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(LogsServiceServer::new(collector))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    (format!("otlp://{address}"), received)
}

/// A custom in-memory stream: a `LogsArgs.Stream` an external author could
/// write, capturing each streamed record's message body. No file, no collector,
/// no console - it straps straight onto the stream interface and reads the capnp
/// records itself.
struct CaptureStream {
    captured: Arc<Mutex<Vec<String>>>,
}

impl logs_args::stream::Server for CaptureStream {
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
            let mut captured = self.captured.lock().unwrap();
            for entry in entries.iter() {
                let Ok(signal::Which::LogRecord(Ok(log_record))) = entry.which() else {
                    continue;
                };
                let Ok(body) = log_record.get_body() else {
                    continue;
                };
                let Ok(any_value::Which::StringValue(Ok(text))) = body.which() else {
                    continue;
                };
                let Ok(text) = text.to_str() else { continue };
                captured.push(text.to_string());
            }
        }
        // Acknowledge so the node frees the batch instead of re-sending it.
        Promise::from_future(async move {
            if let Err(error) = ack.ack_request().send().promise.await {
                eprintln!("capture stream failed to acknowledge a batch: {error}");
            }
            Ok(())
        })
    }

    fn stop(
        &mut self,
        _params: logs_args::stream::StopParams,
        _results: logs_args::stream::StopResults,
    ) -> Promise<(), capnp::Error> {
        // Never stop on our own; the test tears the stream down by killing the
        // process once the marker has arrived.
        Promise::from_future(std::future::pending::<Result<(), capnp::Error>>())
    }
}

// ---- tests ----

#[tokio::test(flavor = "current_thread")]
async fn test_logs_stream_to_custom_stream() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    LocalSet::new()
        .run_until(async move {
            let captured = Arc::new(Mutex::new(Vec::<String>::new()));

            let address: SocketAddr = format!("{LISTEN_ADDRESS}:{port}").parse().unwrap();
            let connection = Connection::connect(address).await.unwrap();
            let client = connection.client().await;

            let builder_captured = captured.clone();
            let program_args = LogsArgs::new(None, FLAG_REPLAY | FLAG_FOLLOW, move || {
                Ok(capnp_rpc::new_client(CaptureStream {
                    captured: builder_captured.clone(),
                }))
            })
            .as_program_args()
            .unwrap();

            let mut process_request = client.process_request();
            program_args
                .with_reader(|reader| process_request.get().set_program_args(reader))
                .unwrap();
            let process = process_request
                .send()
                .promise
                .await
                .unwrap()
                .get()
                .unwrap()
                .get_result()
                .unwrap();

            let pid = process
                .pid_request()
                .send()
                .promise
                .await
                .unwrap()
                .get()
                .unwrap()
                .get_result();

            // Run it as its own task (a daemon), then start the stream by
            // calling its output portal. The logs program streams records to
            // our args server, never to this output sink.
            let mut run_request = client.run_request();
            run_request.get().set_process(process.clone());
            run_request.send().promise.await.unwrap();

            let portal = process
                .portal_request()
                .send()
                .promise
                .await
                .unwrap()
                .get()
                .unwrap()
                .get_result()
                .unwrap()
                .cast_to::<sh_capnp::output_portal::Client>();

            let mut output_request = portal.output_request();
            output_request
                .get()
                .set_stream(capnp_rpc::new_client(Stream::new(OutputSink)));

            let captured_for_predicate = captured.clone();
            let found = tokio::select! {
                result = output_request.send().promise => {
                    // The stream should outlive the wait; if it returned, surface why.
                    result.unwrap();
                    false
                }
                found = wait_until(
                    move || {
                        captured_for_predicate
                            .lock()
                            .unwrap()
                            .iter()
                            .any(|body| body.contains(MARKER))
                    },
                    ARRIVAL_TIMEOUT,
                ) => found,
            };

            let mut kill_request = client.kill_request();
            kill_request.get().set_pid(pid);
            kill_request.get().set_signal(15);
            let _ = kill_request.send().promise.await;
            let _ = connection.disconnect().await;

            assert!(found, "marker never reached the custom in-memory stream");
            assert!(
                !captured.lock().unwrap().is_empty(),
                "no records captured by the custom stream"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_logs_stream_to_http() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    LocalSet::new()
        .run_until(async move {
            let (url, received) = spawn_http_collector().await;

            let received_for_predicate = received.clone();
            let found = drive_logs_stream(
                port,
                &format!("logs stream {url}"),
                Box::new(move || {
                    received_for_predicate
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|record| record.to_string().contains(MARKER))
                }),
            )
            .await?;
            assert!(found, "marker never POSTed to the http collector");
            Ok::<(), anyhow::Error>(())
        })
        .await
        .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn test_logs_stream_to_https() {
    install_crypto_provider();
    // SAFETY: each nextest test runs in its own process; the toggle only makes
    // the streaming client trust the self-signed collector below.
    unsafe { std::env::set_var("DUSK_CLIENT_SKIP_TLS_VERIFY", "1") };

    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    LocalSet::new()
        .run_until(async move {
            let (url, received) = spawn_https_collector().await;

            let received_for_predicate = received.clone();
            let found = drive_logs_stream(
                port,
                &format!("logs stream {url}"),
                Box::new(move || {
                    received_for_predicate
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|record| record.to_string().contains(MARKER))
                }),
            )
            .await?;
            assert!(found, "marker never POSTed to the https collector");
            Ok::<(), anyhow::Error>(())
        })
        .await
        .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn test_logs_stream_to_grpc() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDRESS, port);

    LocalSet::new()
        .run_until(async move {
            let (url, received) = spawn_grpc_collector().await;

            let received_for_predicate = received.clone();
            let found = drive_logs_stream(
                port,
                &format!("logs stream {url}"),
                Box::new(move || {
                    received_for_predicate.lock().unwrap().iter().any(|record| {
                        serde_json::to_string(record)
                            .map(|json| json.contains(MARKER))
                            .unwrap_or(false)
                    })
                }),
            )
            .await?;
            assert!(found, "marker never exported to the gRPC collector");
            Ok::<(), anyhow::Error>(())
        })
        .await
        .unwrap();
}
