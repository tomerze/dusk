use capnp::capability::Promise;

use dusk_capnp::{dusk_capnp::stream, pry};
use dusk_program::value::Value;

use tokio::sync::oneshot;

use crate::display_engine::DisplayEngine;

pub struct DisplayStream<D: DisplayEngine> {
    pub done_sender: Option<oneshot::Sender<()>>,
    display_engine: D,
}

impl<D: DisplayEngine> DisplayStream<D> {
    pub fn new_with_receiver(display_engine: D) -> (Self, oneshot::Receiver<()>) {
        let (done_sender, done_receiver) = oneshot::channel();
        (
            DisplayStream {
                done_sender: Some(done_sender),
                display_engine,
            },
            done_receiver,
        )
    }
}

impl<D: DisplayEngine> stream::Server for DisplayStream<D> {
    fn send(
        &mut self,
        params: dusk_capnp::dusk_capnp::stream::SendParams,
    ) -> Promise<(), capnp::Error> {
        let value = pry!(pry!(params.get()).get_value());
        let value =
            pry!(Value::from_reader(value).map_err(|e| capnp::Error::failed(e.to_string())));
        println!(
            "{}",
            pry!(
                self.display_engine
                    .render_value(value)
                    .map_err(|e| capnp::Error::failed(e.to_string()))
            )
        );
        Promise::ok(())
    }

    fn done(
        &mut self,
        _: dusk_capnp::dusk_capnp::stream::DoneParams,
        _: dusk_capnp::dusk_capnp::stream::DoneResults,
    ) -> Promise<(), capnp::Error> {
        if let Some(done_sender) = self.done_sender.take() {
            pry!(
                done_sender
                    .send(())
                    .map_err(|_| capnp::Error::failed("done signal not sent".to_string()))
            );
            Promise::ok(())
        } else {
            Promise::err(capnp::Error::failed("done already called".to_string()))
        }
    }
}
