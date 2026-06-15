//! The client-side log streams: the consumers of the node's streamed signals.
//! Each stream is an implementation of the `LogsArgs.Server` capnp interface
//! (`send`/`stop`) — the node pushes signals in through [`pipe`](crate::pipe)
//! and the stream writes them wherever it lands. The built-ins live one per
//! submodule; an external author implements the same interface to write their
//! own.
//!
//! A signal is turned into its exportable form by [`convert`](crate::client::convert):
//! the file and http sinks emit OTLP/JSON, the otlp sink the OTLP protobuf types
//! (built from that same JSON). The viewer renders straight off the capnp signals.
//!
//! Backpressure is the `send` itself: a stream resolves its `send` only once it
//! has written the batch (or has room), which fills the node's flow-control
//! window and parks it. A stream that fails fatally triggers its
//! [`stop`](stop_promise) signal — the node's cue to finish.
//!
//! - [`file`] — `file://<path>`: appends one OTLP/JSON signal per line (jsonl).
//! - [`http`] — `http://` / `https://`: one POST per signal, OTLP/JSON.
//! - [`otlp`] — `otlp://<host:port>`: OTLP/gRPC `Export` calls, one per batch.
//! - [`viewer`] — the interactive terminal pager (no url).
//! - [`print`] — a plain stdout dump (no url, `--replay-only`).

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use crate::logs_capnp::logs_args;
use capnp::capability::Promise;
use dusk_program::anyhow::{Result, bail};
use dusk_program::dusk_capnp::capnp_rpc;
use tokio::sync::Notify;

pub(crate) const RETRY_INTERVAL: Duration = Duration::from_millis(500);

pub mod file;
pub mod http;
pub mod otlp;
pub mod print;
pub mod viewer;

pub use file::FileStream;
pub use http::HttpStream;
pub use otlp::OtlpStream;
pub use print::PrintStream;
pub use viewer::ViewerStream;

/// Build the built-in stream for a `logs stream` URL. The viewer has no url; it
/// is constructed directly ([`ViewerStream`]).
pub(crate) fn parse(url: &str, namespace_id: u64) -> Result<logs_args::server::Client> {
    let server: logs_args::server::Client = if let Some(path) = url.strip_prefix("file://") {
        if path.is_empty() {
            bail!("file:// needs a path: {url}");
        }
        capnp_rpc::new_client(FileStream::new(PathBuf::from(path), namespace_id))
    } else if let Some(authority) = url.strip_prefix("otlp://") {
        if authority.is_empty() {
            bail!("otlp:// needs a host:port: {url}");
        }
        capnp_rpc::new_client(OtlpStream::new(format!("http://{authority}"), namespace_id))
    } else if url.starts_with("http://") || url.starts_with("https://") {
        capnp_rpc::new_client(HttpStream::new(url.to_string(), namespace_id))
    } else {
        bail!("unsupported url: {url} (expected file://, otlp://, http:// or https://)")
    };
    Ok(server)
}

/// Answer the node's `stop` long-poll once `stop` is signalled — the cue to
/// finish the stream. Shared by every built-in: a stream signals it when it
/// fails fatally or (the viewer) when the user quits.
pub(crate) fn stop_promise(stop: &Rc<Notify>) -> Promise<(), capnp::Error> {
    let stop = stop.clone();
    Promise::from_future(async move {
        stop.notified().await;
        Ok(())
    })
}
