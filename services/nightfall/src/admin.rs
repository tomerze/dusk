use crate::directory::{LocalEntry, namespace_hex, parse_namespace};
use crate::peer::PrincipalCertificate;
use crate::server::Shared;
use crate::shard::Control;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{CONTENT_TYPE, HeaderValue};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use serde_json::{Value, json};
use std::convert::Infallible;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub const MAXIMUM_CONNECTIONS: usize = 64;
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const SHARD_REPLY_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_PAGE: usize = 100;
pub const MAXIMUM_PAGE: usize = 1000;
pub const ADMIN_ROLE: &str = "admin";
const MAXIMUM_BODY_BYTES: usize = 64 * 1024;

static METRICS: OnceLock<PrometheusHandle> = OnceLock::new();

pub fn install_metrics() -> PrometheusHandle {
    METRICS
        .get_or_init(|| {
            let recorder = PrometheusBuilder::new().build_recorder();
            let handle = recorder.handle();
            if let Err(error) = metrics::set_global_recorder(recorder) {
                tracing::warn!(%error, "a metrics recorder was already installed; /metrics shows only nightfall's own recorder");
            }
            handle
        })
        .clone()
}

type Body = Full<Bytes>;

fn respond(
    status: StatusCode,
    content_type: &'static str,
    body: impl Into<Bytes>,
) -> Response<Body> {
    let mut response = Response::new(Full::new(body.into()));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

fn json_response(status: StatusCode, value: &Value) -> Response<Body> {
    respond(status, "application/json", value.to_string())
}

fn error(status: StatusCode, message: &str) -> Response<Body> {
    json_response(status, &json!({ "error": message }))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caller {
    Anonymous,
    Principal(PrincipalCertificate),
}

impl Caller {
    fn name(&self) -> &str {
        match self {
            Caller::Anonymous => "anonymous",
            Caller::Principal(principal) => &principal.principal,
        }
    }
}

fn query_value<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (key == name).then_some(value)
    })
}

pub fn list_sessions(shared: &Shared, query: &str) -> Result<Value, String> {
    let limit = match query_value(query, "limit") {
        Some(text) => text
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=MAXIMUM_PAGE).contains(limit))
            .ok_or_else(|| format!("limit must be between 1 and {MAXIMUM_PAGE}"))?,
        None => DEFAULT_PAGE,
    };
    let after = match query_value(query, "after") {
        Some(text) => Some(
            parse_namespace(text)
                .ok_or("after must be a namespace id of 16 lowercase hex digits")?,
        ),
        None => None,
    };
    let mut page: Vec<(u64, LocalEntry)> = {
        let directory = shared.directory();
        let mut candidates: Vec<u64> = directory
            .local_entries()
            .map(|(namespace_id, _)| *namespace_id)
            .filter(|namespace_id| after.is_none_or(|after| *namespace_id > after))
            .collect();
        if candidates.len() > limit + 1 {
            candidates.select_nth_unstable(limit);
            candidates.truncate(limit + 1);
        }
        candidates
            .into_iter()
            .filter_map(|namespace_id| {
                directory
                    .local(namespace_id)
                    .map(|entry| (namespace_id, entry.clone()))
            })
            .collect()
    };
    page.sort_by_key(|(namespace_id, _)| *namespace_id);
    let mut sessions: Vec<(u64, Value)> = page
        .into_iter()
        .map(|(namespace_id, entry)| {
            (
                namespace_id,
                json!({
                    "namespace_id": namespace_hex(namespace_id),
                    "device_id": entry.identity.device_id(),
                    "installation_id": entry.identity.installation_id(),
                    "epoch": entry.epoch,
                    "shard": entry.shard,
                    "tenant": entry.tenant,
                    "remote_address": entry.remote_address.to_string(),
                    "connected_at": entry.connected_at,
                    "quarantined": entry.quarantined,
                }),
            )
        })
        .collect();
    let more = sessions.len() > limit;
    sessions.truncate(limit);
    let next = if more {
        sessions
            .last()
            .map(|(namespace_id, _)| namespace_hex(*namespace_id))
    } else {
        None
    };
    Ok(json!({
        "sessions": sessions.into_iter().map(|(_, session)| session).collect::<Vec<_>>(),
        "next": next,
    }))
}

fn owner(shared: &Shared, namespace_id: u64) -> Option<(usize, u64)> {
    shared
        .directory()
        .local(namespace_id)
        .map(|entry| (entry.shard, entry.epoch))
}

async fn ask(
    shared: &Shared,
    shard: usize,
    control: impl FnOnce(oneshot::Sender<Option<Value>>) -> Control,
) -> Option<Value> {
    let (reply, answer) = oneshot::channel();
    shared.send(shard, control(reply));
    tokio::time::timeout(SHARD_REPLY_TIMEOUT, answer)
        .await
        .ok()
        .and_then(Result::ok)
        .flatten()
}

fn refusal(shared: &Shared, caller: &Caller) -> Option<&'static str> {
    let Caller::Principal(principal) = caller else {
        return Some("no client certificate names a principal");
    };
    let permissions = shared.permissions.load();
    if permissions.denies_certificate(&principal.fingerprint_bytes) {
        metrics::counter!("nightfall_principals_refused_total").increment(1);
        return Some("its certificate is denied");
    }
    let admin = permissions
        .policy_for(&principal.principal, false)
        .is_some_and(|policy| policy.role_names().contains(&ADMIN_ROLE));
    (!admin).then_some("the principal has no admin role")
}

async fn administer(
    shared: &Shared,
    caller: &Caller,
    method: &Method,
    path: &[&str],
    query: &str,
) -> Response<Body> {
    match (method, path) {
        (&Method::GET, ["v1", "sessions"]) => match list_sessions(shared, query) {
            Ok(value) => json_response(StatusCode::OK, &value),
            Err(message) => error(StatusCode::BAD_REQUEST, &message),
        },
        (&Method::GET, ["v1", "sessions", namespace])
        | (&Method::GET, ["v1", "sessions", namespace, "provenance"]) => {
            let Some(namespace_id) = parse_namespace(namespace) else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "a namespace id is 16 lowercase hex digits",
                );
            };
            let Some((shard, _)) = owner(shared, namespace_id) else {
                return error(
                    StatusCode::NOT_FOUND,
                    "this instance holds no session of that namespace",
                );
            };
            let provenance = path.len() == 4;
            let answer = ask(shared, shard, |reply| {
                if provenance {
                    Control::Provenance {
                        namespace_id,
                        reply,
                    }
                } else {
                    Control::Describe {
                        namespace_id,
                        reply,
                    }
                }
            })
            .await;
            match answer {
                Some(value) => json_response(StatusCode::OK, &value),
                None => error(
                    StatusCode::NOT_FOUND,
                    "this instance holds no session of that namespace",
                ),
            }
        }
        (&Method::POST, ["v1", "sessions", namespace, "kill"]) => {
            let Some(namespace_id) = parse_namespace(namespace) else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "a namespace id is 16 lowercase hex digits",
                );
            };
            let Some((shard, epoch)) = owner(shared, namespace_id) else {
                return error(
                    StatusCode::NOT_FOUND,
                    "this instance holds no session of that namespace",
                );
            };
            tracing::warn!(namespace_id = %namespace, epoch, by = caller.name(), "an admin killed a node session");
            shared.send(
                shard,
                Control::Kill {
                    namespace_id,
                    epoch,
                    by: caller.name().to_string(),
                },
            );
            json_response(
                StatusCode::ACCEPTED,
                &json!({ "namespace_id": namespace, "epoch": epoch }),
            )
        }
        (&Method::POST, ["v1", "clients", session_id, "kill"]) => {
            let mut answers = Vec::with_capacity(shared.shards.len());
            for shard in 0..shared.shards.len() {
                let (reply, answer) = oneshot::channel();
                shared.send(
                    shard,
                    Control::KillClient {
                        session_id: session_id.to_string(),
                        by: caller.name().to_string(),
                        reply,
                    },
                );
                answers.push(answer);
            }
            let mut killed = false;
            for answer in answers {
                killed |= tokio::time::timeout(SHARD_REPLY_TIMEOUT, answer)
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or(false);
            }
            if killed {
                tracing::warn!(
                    session_id,
                    by = caller.name(),
                    "an admin killed a client session"
                );
                json_response(StatusCode::ACCEPTED, &json!({ "session_id": session_id }))
            } else {
                error(
                    StatusCode::NOT_FOUND,
                    "this instance holds no client session with that id",
                )
            }
        }
        _ => error(StatusCode::NOT_FOUND, "no such endpoint"),
    }
}

async fn handle(
    shared: Arc<Shared>,
    metrics: PrometheusHandle,
    caller: Caller,
    request: Request<Incoming>,
) -> Response<Body> {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let query = request.uri().query().unwrap_or_default().to_string();
    if let Err(failure) = Limited::new(request.into_body(), MAXIMUM_BODY_BYTES)
        .collect()
        .await
    {
        tracing::debug!(%failure, "an admin request body was refused");
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "the request body is too large",
        );
    }
    match (&method, path.as_str()) {
        (&Method::GET, "/healthz") => return respond(StatusCode::OK, "text/plain", "ok\n"),
        (&Method::GET, "/readyz") => {
            return match shared.ready() {
                Ok(()) => respond(StatusCode::OK, "text/plain", "ready\n"),
                Err(reason) => respond(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "text/plain",
                    format!("not ready: {reason}\n"),
                ),
            };
        }
        (&Method::GET, "/metrics") => {
            return respond(
                StatusCode::OK,
                "text/plain; version=0.0.4",
                metrics.render(),
            );
        }
        _ => {}
    }
    if !path.starts_with("/v1/") {
        return error(StatusCode::NOT_FOUND, "no such endpoint");
    }
    if shared.tls.admin.is_none() {
        return error(StatusCode::NOT_FOUND, "the admin API is disabled");
    }
    if let Some(reason) = refusal(&shared, &caller) {
        tracing::warn!(caller = caller.name(), reason, %method, path, "refused an admin request");
        return error(StatusCode::FORBIDDEN, "the admin role is required");
    }
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    let response = administer(&shared, &caller, &method, &segments, &query).await;
    tracing::info!(caller = caller.name(), %method, path, status = response.status().as_u16(), "admin request");
    response
}

async fn serve_connection<Stream>(
    shared: Arc<Shared>,
    metrics: PrometheusHandle,
    caller: Caller,
    stream: Stream,
) where
    Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let service = hyper::service::service_fn(move |request| {
        let shared = shared.clone();
        let metrics = metrics.clone();
        let caller = caller.clone();
        async move {
            let response =
                tokio::time::timeout(REQUEST_TIMEOUT, handle(shared, metrics, caller, request))
                    .await
                    .unwrap_or_else(|_| {
                        error(StatusCode::GATEWAY_TIMEOUT, "the request took too long")
                    });
            Ok::<_, Infallible>(response)
        }
    });
    if let Err(failure) = hyper::server::conn::http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_TIMEOUT)
        .keep_alive(true)
        .serve_connection(TokioIo::new(stream), service)
        .await
    {
        tracing::debug!(%failure, "an admin connection failed");
    }
}

fn caller_of(connection: &rustls::ServerConnection) -> Caller {
    let Some(certificate) = connection
        .peer_certificates()
        .and_then(|certificates| certificates.first())
    else {
        return Caller::Anonymous;
    };
    match crate::peer::principal_certificate(certificate) {
        Ok(principal) => Caller::Principal(principal),
        Err(failure) => {
            tracing::warn!(
                error = format!("{failure:#}"),
                "an admin client certificate names no principal"
            );
            Caller::Anonymous
        }
    }
}

pub async fn serve(shared: Arc<Shared>, listener: std::net::TcpListener, stop: CancellationToken) {
    let metrics = install_metrics();
    let listener = match listener
        .set_nonblocking(true)
        .and_then(|()| tokio::net::TcpListener::from_std(listener))
    {
        Ok(listener) => listener,
        Err(failure) => {
            tracing::error!(%failure, "the admin listener could not start");
            return;
        }
    };
    let connections = Arc::new(tokio::sync::Semaphore::new(MAXIMUM_CONNECTIONS));
    loop {
        let accepted = tokio::select! {
            () = stop.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        let (stream, remote) = match accepted {
            Ok(accepted) => accepted,
            Err(failure) => {
                tracing::warn!(%failure, "accepting an admin connection failed");
                tokio::time::sleep(crate::listener::ACCEPT_PAUSE).await;
                continue;
            }
        };
        let Ok(permit) = connections.clone().try_acquire_owned() else {
            tracing::warn!(%remote, "refused an admin connection: too many are open");
            continue;
        };
        let shared = shared.clone();
        let metrics = metrics.clone();
        tokio::spawn(async move {
            let _permit = permit;
            match shared.tls.admin.clone() {
                None => serve_connection(shared, metrics, Caller::Anonymous, stream).await,
                Some(config) => {
                    let accepted = tokio::time::timeout(
                        HEADER_TIMEOUT,
                        tokio_rustls::TlsAcceptor::from(config).accept(stream),
                    )
                    .await;
                    match accepted {
                        Ok(Ok(tls)) => {
                            let caller = caller_of(tls.get_ref().1);
                            serve_connection(shared, metrics, caller, tls).await;
                        }
                        Ok(Err(failure)) => {
                            tracing::debug!(%remote, %failure, "an admin TLS handshake failed")
                        }
                        Err(_) => tracing::debug!(%remote, "an admin TLS handshake timed out"),
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_query_values() {
        assert_eq!(
            query_value("limit=5&after=00000000000000ab", "after"),
            Some("00000000000000ab")
        );
        assert_eq!(query_value("limit=5", "after"), None);
        assert_eq!(query_value("flag&limit=5", "flag"), Some(""));
    }
}
