use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::stream;

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
