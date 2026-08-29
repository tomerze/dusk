use capnp::capability::Promise;
use std::println;
use std::string::ToString;

use dusk_program::stream::{Stream, StreamMixin};
use dusk_program::value::Value;

use tokio::sync::oneshot;

use crate::client::prompt::display_engine::DisplayEngine;

pub struct DisplayStream<D: DisplayEngine> {
    pub done_sender: Option<oneshot::Sender<()>>,
    display_engine: D,
}

impl<D: DisplayEngine> DisplayStream<D> {
    pub fn new_with_receiver(display_engine: D) -> (Stream<Self>, oneshot::Receiver<()>) {
        let (done_sender, done_receiver) = oneshot::channel();
        (
            Stream::new(DisplayStream {
                done_sender: Some(done_sender),
                display_engine,
            }),
            done_receiver,
        )
    }
}

impl<D: DisplayEngine> StreamMixin for DisplayStream<D> {
    fn send(&mut self, value: Value) -> Promise<(), capnp::Error> {
        match self.display_engine.render_value(value) {
            Ok(rendered) => {
                println!("{}", rendered);
                Promise::ok(())
            }
            Err(error) => Promise::err(capnp::Error::failed(error.to_string())),
        }
    }

    fn end(&mut self) {
        if let Some(done_sender) = self.done_sender.take() {
            let _ = done_sender.send(());
        }
    }
}
