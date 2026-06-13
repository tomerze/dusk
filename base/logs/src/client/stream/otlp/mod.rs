//! `otlp://<host:port>`: OTLP/gRPC `Export` calls to a collector, one per batch.

mod convert;

use super::stop_promise;
use crate::logs_capnp::logs_args;
use capnp::capability::Promise;
use dusk_program::anyhow::{Context as _, Result};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::logs::v1::logs_service_client::LogsServiceClient;
use opentelemetry_proto::tonic::logs::v1 as otlp_logs;
use std::rc::Rc;
use std::string::String;
use std::vec::Vec;
use tokio::sync::{Mutex, Notify};

/// The collector, connected lazily on the first batch so construction stays
/// sync. A fatal failure latches [`State::Failed`] so later batches drop
/// rather than retry a dead connection.
enum State {
    Unconnected(String),
    Connected {
        endpoint: String,
        client: LogsServiceClient<tonic::transport::Channel>,
    },
    Failed,
}

/// A `LogsArgs.Server` stream that exports each batch to an OTLP/gRPC collector.
pub struct OtlpStream {
    state: Rc<Mutex<State>>,
    stop: Rc<Notify>,
}

impl OtlpStream {
    /// `endpoint` is already in the `http://host:port` form tonic dials.
    pub fn new(endpoint: String) -> Self {
        OtlpStream {
            state: Rc::new(Mutex::new(State::Unconnected(endpoint))),
            stop: Rc::new(Notify::new()),
        }
    }
}

impl logs_args::server::Server for OtlpStream {
    fn send(&mut self, params: logs_args::server::SendParams) -> Promise<(), capnp::Error> {
        let entries = dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_entries());
        let batch = convert::records(entries);
        let state = self.state.clone();
        let stop = self.stop.clone();
        Promise::from_future(async move {
            if let Err(error) = export(&state, batch).await {
                tracing::error!(error = %format!("{error:#}"), "the otlp log stream failed");
                stop.notify_one();
            }
            Ok(())
        })
    }

    fn stop(
        &mut self,
        _params: logs_args::server::StopParams,
        _results: logs_args::server::StopResults,
    ) -> Promise<(), capnp::Error> {
        stop_promise(&self.stop)
    }
}

/// Connect on first use, then export `batch` in one `Export` call. Awaiting the
/// export is the backpressure — a slow collector parks the node's stream.
async fn export(state: &Rc<Mutex<State>>, batch: Vec<otlp_logs::LogRecord>) -> Result<()> {
    let mut guard = state.lock().await;
    if let State::Unconnected(endpoint) = &*guard {
        let endpoint = endpoint.clone();
        match LogsServiceClient::connect(endpoint.clone()).await {
            Ok(client) => *guard = State::Connected { endpoint, client },
            Err(error) => {
                *guard = State::Failed;
                return Err(error).with_context(|| format!("connecting to {endpoint}"));
            }
        }
    }
    let State::Connected { endpoint, client } = &mut *guard else {
        return Ok(());
    };
    let request = ExportLogsServiceRequest {
        resource_logs: vec![otlp_logs::ResourceLogs {
            resource: None,
            scope_logs: vec![otlp_logs::ScopeLogs {
                scope: None,
                log_records: batch,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    let response = client
        .export(request)
        .await
        .with_context(|| format!("exporting to {endpoint}"))?;
    if let Some(partial) = response.into_inner().partial_success
        && partial.rejected_log_records > 0
    {
        tracing::warn!(
            rejected = partial.rejected_log_records,
            message = %partial.error_message,
            "the collector rejected log records"
        );
    }
    Ok(())
}
