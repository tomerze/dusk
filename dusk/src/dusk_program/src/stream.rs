use alloc::string::ToString;
use dusk_capnp::capnp;
use dusk_capnp::capnp::capability::Promise;
use dusk_capnp::dusk_capnp::stream;
use dusk_capnp::pry;
use oneshot::{Receiver, Sender};

#[derive(Default)]
pub struct NoopStream {}

impl NoopStream {
    pub fn new() -> Self {
        NoopStream {}
    }
}

impl stream::Server for NoopStream {
    fn send(&mut self, _: stream::SendParams) -> capnp::capability::Promise<(), capnp::Error> {
        Promise::ok(())
    }

    fn done(
        &mut self,
        _: stream::DoneParams,
        _: stream::DoneResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        Promise::ok(())
    }
}

pub struct UndoneStream {
    stream: stream::Client,
    done_sender: Option<Sender<()>>,
}

/// Wraps a stream but does not call done on it, instead it notifies the caller when done is called.
impl UndoneStream {
    pub fn new_with_done_receiver(stream: stream::Client) -> (Self, Receiver<()>) {
        let (done_sender, done_receiver) = oneshot::channel();
        (
            UndoneStream {
                stream,
                done_sender: Some(done_sender),
            },
            done_receiver,
        )
    }
}

impl stream::Server for UndoneStream {
    fn send(&mut self, params: stream::SendParams) -> capnp::capability::Promise<(), capnp::Error> {
        let value = pry!(pry!(params.get()).get_value());
        let mut send_request = self.stream.send_request();
        pry!(send_request.get().set_value(value));
        Promise::from_future(async move { send_request.send().await })
    }

    fn done(
        &mut self,
        _: stream::DoneParams,
        _: stream::DoneResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        // And here is the real magic, we don't call done on the underlying stream.
        // Instead we just notify the caller that done was called.
        let sender = self.done_sender.take();
        if let Some(done_sender) = sender {
            done_sender
                .send(())
                .map_err(|_| capnp::Error::failed("failed to send done signal".to_string()))
                .into()
        } else {
            Promise::err(capnp::Error::failed("done already called".to_string()))
        }
    }
}
