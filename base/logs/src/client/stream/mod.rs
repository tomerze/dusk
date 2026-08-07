use std::path::PathBuf;

use crate::logs_capnp::logs_args;
use crate::logs_capnp::signal_batch;
use dusk_program::anyhow::{Result, bail};
use dusk_program::dusk_capnp::capnp_rpc;

pub mod file;
pub mod http;
pub mod otlp;
pub mod viewer;

pub use file::FileStream;
pub use http::HttpStream;
pub use otlp::OtlpStream;
pub use viewer::ViewerStream;

pub type StreamBuilder = Box<dyn Fn() -> Result<logs_args::stream::Client>>;

pub(crate) fn parse(url: &str, namespace_id: u64) -> Result<StreamBuilder> {
    if let Some(path) = url.strip_prefix("file://") {
        if path.is_empty() {
            bail!("file:// needs a path: {url}");
        }
        let path = PathBuf::from(path);
        Ok(Box::new(move || {
            Ok(capnp_rpc::new_client(FileStream::new(
                path.clone(),
                namespace_id,
            )))
        }))
    } else if let Some(authority) = url.strip_prefix("otlp://") {
        if authority.is_empty() {
            bail!("otlp:// needs a host:port: {url}");
        }
        let endpoint = format!("http://{authority}");
        Ok(Box::new(move || {
            Ok(capnp_rpc::new_client(OtlpStream::new(
                endpoint.clone(),
                namespace_id,
            )))
        }))
    } else if url.starts_with("http://") || url.starts_with("https://") {
        let url = url.to_string();
        Ok(Box::new(move || {
            Ok(capnp_rpc::new_client(HttpStream::new(
                url.clone(),
                namespace_id,
            )))
        }))
    } else {
        bail!("unsupported url: {url} (expected file://, otlp://, http:// or https://)")
    }
}

pub(crate) async fn acknowledge(ack: signal_batch::ack::Client) {
    if let Err(error) = ack.ack_request().send().promise.await {
        tracing::warn!(%error, "failed to acknowledge a streamed batch");
    }
}
