use std::path::PathBuf;

use crate::logs_capnp::logs_args;
use crate::logs_capnp::signal_batch;
use dusk_program::anyhow::{Result, bail};
use dusk_program::dusk_capnp::capnp_rpc;

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

pub(crate) async fn acknowledge(ack: signal_batch::ack::Client) {
    if let Err(error) = ack.ack_request().send().promise.await {
        tracing::warn!(%error, "failed to acknowledge a streamed batch");
    }
}
