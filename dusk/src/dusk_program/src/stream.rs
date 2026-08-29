use alloc::string::ToString;
use dusk_capnp::capnp;
use dusk_capnp::capnp::capability::Promise;
use dusk_capnp::dusk_capnp::stream;
use dusk_capnp::pry;

use crate::value::Value;

pub trait StreamMixin {
    fn send(&mut self, value: Value) -> Promise<(), capnp::Error>;

    fn end(&mut self);
}

pub struct Stream<M: StreamMixin> {
    mixin: M,
    ended: bool,
}

impl<M: StreamMixin> Stream<M> {
    pub fn new(mixin: M) -> Self {
        Stream {
            mixin,
            ended: false,
        }
    }

    fn end(&mut self) {
        if !self.ended {
            self.ended = true;
            self.mixin.end();
        }
    }
}

impl<M: StreamMixin> stream::Server for Stream<M> {
    fn send(&mut self, params: stream::SendParams) -> Promise<(), capnp::Error> {
        let value = pry!(pry!(params.get()).get_value());
        let value =
            pry!(Value::from_reader(value).map_err(|e| capnp::Error::failed(e.to_string())));
        self.mixin.send(value)
    }

    fn done(&mut self, _: stream::DoneParams, _: stream::DoneResults) -> Promise<(), capnp::Error> {
        self.end();
        Promise::ok(())
    }
}

impl<M: StreamMixin> Drop for Stream<M> {
    fn drop(&mut self) {
        self.end();
    }
}

#[derive(Default)]
pub struct NoopStream {}

impl NoopStream {
    pub fn new() -> Stream<Self> {
        Stream::new(NoopStream {})
    }
}

impl StreamMixin for NoopStream {
    fn send(&mut self, _: Value) -> Promise<(), capnp::Error> {
        Promise::ok(())
    }

    fn end(&mut self) {}
}
