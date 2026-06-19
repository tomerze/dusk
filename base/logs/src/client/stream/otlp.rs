use super::acknowledge;
use crate::client::convert::signal_to_json;
use crate::logs_capnp::{logs_args, signal};
use capnp::capability::Promise;
use dusk_program::anyhow::{Context as _, Result};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::logs::v1::logs_service_client::LogsServiceClient;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::trace_service_client::TraceServiceClient;
use opentelemetry_proto::tonic::common::v1 as otlp_common;
use opentelemetry_proto::tonic::logs::v1 as otlp_logs;
use opentelemetry_proto::tonic::resource::v1 as otlp_resource;
use opentelemetry_proto::tonic::trace::v1 as otlp_trace;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::string::String;
use std::vec::Vec;
use tokio::sync::{Mutex, Notify};
use tonic::transport::Channel;

/// A span's `program_name` attribute names its service; a span with no program
/// belongs to the node core.
const PROGRAM_NAME_FIELD: &str = "program_name";
const CORE_SERVICE: &str = "core";

/// The collector connection, dialed lazily on the first batch so construction
/// stays sync. One channel backs both signal clients. A failed connect or export
/// drops back to [`State::Unconnected`] so the next attempt redials.
enum State {
    Unconnected(String),
    Connected {
        endpoint: String,
        logs: Box<LogsServiceClient<Channel>>,
        traces: Box<TraceServiceClient<Channel>>,
    },
}

/// A `LogsArgs.Server` stream that exports each batch to an OTLP/gRPC collector —
/// logs on the log signal, reconstructed spans on the trace signal.
pub struct OtlpStream {
    state: Rc<Mutex<State>>,
    stop: Rc<Notify>,
    namespace_id: u64,
}

impl OtlpStream {
    /// `endpoint` is already in the `http://host:port` form tonic dials.
    pub fn new(endpoint: String, namespace_id: u64) -> Self {
        OtlpStream {
            state: Rc::new(Mutex::new(State::Unconnected(endpoint))),
            stop: Rc::new(Notify::new()),
            namespace_id,
        }
    }
}

impl Drop for OtlpStream {
    fn drop(&mut self) {
        // The node finished the stream or the connection died; there is no UI to
        // show it, so the close is the log.
        tracing::info!("the otlp stream closed");
    }
}

impl logs_args::server::Server for OtlpStream {
    fn send(&mut self, params: logs_args::server::SendParams) -> Promise<(), capnp::Error> {
        let signal_batch = dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_signal_batch());
        let entries = dusk_capnp::pry!(signal_batch.get_signals());
        let ack = dusk_capnp::pry!(signal_batch.get_ack());

        let mut logs: Vec<otlp_logs::LogRecord> = Vec::new();
        let mut spans: Vec<otlp_trace::Span> = Vec::new();
        for entry in entries {
            let json = match signal_to_json(entry, self.namespace_id) {
                Ok(json) => json,
                Err(error) => {
                    tracing::warn!(%error, "skipping an unconvertible signal");
                    continue;
                }
            };
            match entry.which() {
                Ok(signal::Which::LogRecord(_)) => match serde_json::from_value(json) {
                    Ok(log) => logs.push(log),
                    Err(error) => tracing::warn!(%error, "skipping an unconvertible log record"),
                },
                Ok(signal::Which::Span(_)) => match serde_json::from_value(json) {
                    Ok(span) => spans.push(span),
                    Err(error) => tracing::warn!(%error, "skipping an unconvertible span"),
                },
                Err(error) => tracing::warn!(%error, "skipping an unreadable signal"),
            }
        }
        let logs_count = logs.len();
        let spans_count = spans.len();
        let resource_spans = resource_spans(spans);

        let state = self.state.clone();
        Promise::from_future(async move {
            if let Err(error) = export(&state, &logs, &resource_spans).await {
                let message = format!("{error:#}");
                tracing::warn!(logs_count, spans_count, error = %message, "failed streaming over otlp");
                return Ok(());
            }
            acknowledge(ack).await;
            tracing::info!(logs_count, spans_count, "streaming over otlp...");
            Ok(())
        })
    }

    fn stop(
        &mut self,
        _params: logs_args::server::StopParams,
        _results: logs_args::server::StopResults,
    ) -> Promise<(), capnp::Error> {
        let stop = self.stop.clone();
        Promise::from_future(async move {
            stop.notified().await;
            Ok(())
        })
    }
}

/// Dial the collector and build both signal clients over one channel.
async fn connect(
    endpoint: &str,
) -> Result<(LogsServiceClient<Channel>, TraceServiceClient<Channel>)> {
    let channel = tonic::transport::Endpoint::from_shared(endpoint.to_string())
        .with_context(|| format!("invalid otlp endpoint {endpoint}"))?
        .connect()
        .await
        .with_context(|| format!("connecting to {endpoint}"))?;
    Ok((
        LogsServiceClient::new(channel.clone()),
        TraceServiceClient::new(channel),
    ))
}

/// Group spans into OTLP `ResourceSpans`, one per service — a service per program
/// (`program_name`), program-less spans under `core`.
fn resource_spans(spans: Vec<otlp_trace::Span>) -> Vec<otlp_trace::ResourceSpans> {
    let mut by_service: BTreeMap<String, Vec<otlp_trace::Span>> = BTreeMap::new();
    for span in spans {
        by_service.entry(service_of(&span)).or_default().push(span);
    }
    by_service
        .into_iter()
        .map(|(service, spans)| otlp_trace::ResourceSpans {
            resource: Some(otlp_resource::Resource {
                attributes: vec![otlp_common::KeyValue {
                    key: "service.name".to_string(),
                    value: Some(otlp_common::AnyValue {
                        value: Some(otlp_common::any_value::Value::StringValue(service)),
                    }),
                }],
                ..Default::default()
            }),
            scope_spans: vec![otlp_trace::ScopeSpans {
                scope: None,
                spans,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        })
        .collect()
}

/// A span's service: its `program_name` attribute, or `core` if it carries none.
fn service_of(span: &otlp_trace::Span) -> String {
    for attribute in &span.attributes {
        if attribute.key == PROGRAM_NAME_FIELD
            && let Some(otlp_common::AnyValue {
                value: Some(otlp_common::any_value::Value::StringValue(name)),
            }) = &attribute.value
        {
            return name.clone();
        }
    }
    CORE_SERVICE.to_string()
}

/// Connect on first use, then export this batch's logs and spans. Awaiting the
/// exports is the backpressure — a slow collector parks the node's stream. On any
/// failure the connection is dropped back to [`State::Unconnected`] so the
/// caller's retry redials.
async fn export(
    state: &Rc<Mutex<State>>,
    logs: &[otlp_logs::LogRecord],
    resource_spans: &[otlp_trace::ResourceSpans],
) -> Result<()> {
    let mut guard = state.lock().await;
    if let State::Unconnected(endpoint) = &*guard {
        let endpoint = endpoint.clone();
        // A failed connect leaves the state Unconnected, so the retry redials.
        let (logs_client, traces_client) = connect(&endpoint).await?;
        *guard = State::Connected {
            endpoint,
            logs: Box::new(logs_client),
            traces: Box::new(traces_client),
        };
    }
    let State::Connected {
        endpoint,
        logs: logs_client,
        traces: traces_client,
    } = &mut *guard
    else {
        return Ok(());
    };

    let result = export_signals(endpoint, logs_client, traces_client, logs, resource_spans).await;
    if result.is_err() {
        // The connection may be the casualty; drop it so the retry redials.
        let endpoint = endpoint.clone();
        *guard = State::Unconnected(endpoint);
    }
    result
}

/// One export attempt over an established connection: this batch's logs on the
/// log signal, its spans on the trace signal.
async fn export_signals(
    endpoint: &str,
    logs_client: &mut LogsServiceClient<Channel>,
    traces_client: &mut TraceServiceClient<Channel>,
    logs: &[otlp_logs::LogRecord],
    resource_spans: &[otlp_trace::ResourceSpans],
) -> Result<()> {
    if !logs.is_empty() {
        let request = ExportLogsServiceRequest {
            resource_logs: vec![otlp_logs::ResourceLogs {
                resource: None,
                scope_logs: vec![otlp_logs::ScopeLogs {
                    scope: None,
                    log_records: logs.to_vec(),
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        let response = logs_client
            .export(request)
            .await
            .with_context(|| format!("exporting logs to {endpoint}"))?;
        if let Some(partial) = response.into_inner().partial_success
            && partial.rejected_log_records > 0
        {
            tracing::warn!(
                rejected = partial.rejected_log_records,
                message = %partial.error_message,
                "the collector rejected log records"
            );
        }
    }

    if !resource_spans.is_empty() {
        let request = ExportTraceServiceRequest {
            resource_spans: resource_spans.to_vec(),
        };
        let response = traces_client
            .export(request)
            .await
            .with_context(|| format!("exporting spans to {endpoint}"))?;
        if let Some(partial) = response.into_inner().partial_success
            && partial.rejected_spans > 0
        {
            tracing::warn!(
                rejected = partial.rejected_spans,
                message = %partial.error_message,
                "the collector rejected spans"
            );
        }
    }
    Ok(())
}
