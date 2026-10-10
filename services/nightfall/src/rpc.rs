use capnp::message::ReaderOptions;
use capnp_rpc::rpc_twoparty_capnp::Side;
use capnp_rpc::{RpcSystem, twoparty};
use nightfall_membrane::filter::filter_vat_network;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

pub const NESTING_LIMIT: i32 = 64;

pub fn reader_options(max_message_bytes: u64) -> ReaderOptions {
    ReaderOptions {
        traversal_limit_in_words: Some((max_message_bytes / 8) as usize),
        nesting_limit: NESTING_LIMIT,
    }
}

pub fn system<Stream>(
    stream: Stream,
    side: Side,
    max_message_bytes: u64,
    bootstrap: Option<capnp::capability::Client>,
    observer: impl Fn(&str) + 'static,
) -> RpcSystem<Side>
where
    Stream: AsyncRead + AsyncWrite + Unpin + 'static,
{
    let (reader, writer) = tokio::io::split(stream);
    let network = filter_vat_network(
        twoparty::VatNetwork::new(
            reader.compat(),
            writer.compat_write(),
            side,
            reader_options(max_message_bytes),
        ),
        observer,
    );
    RpcSystem::new(Box::new(network), bootstrap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_traversal_by_the_message_limit() {
        let options = reader_options(4 * 1024 * 1024);
        assert_eq!(options.traversal_limit_in_words, Some(512 * 1024));
        assert_eq!(options.nesting_limit, NESTING_LIMIT);
    }
}
