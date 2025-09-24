use capnp::capability::Promise;
use dusk_capnp::{dusk_capnp::stream::Server, pry};
use tokio::sync::oneshot;

pub struct DisplayStream {
    pub done_sender: Option<oneshot::Sender<()>>,
}

impl DisplayStream {
    pub fn new_with_receiver() -> (Self, oneshot::Receiver<()>) {
        let (done_sender, done_receiver) = oneshot::channel();
        (
            DisplayStream {
                done_sender: Some(done_sender),
            },
            done_receiver,
        )
    }

    fn value_to_string(value: dusk_capnp::dusk_capnp::value::Reader<'_>) -> String {
        // TODO: fix this
        match value.which() {
            Ok(dusk_capnp::dusk_capnp::value::Text(_s)) => "".to_string(),
            Ok(dusk_capnp::dusk_capnp::value::Int(i)) => i.to_string(),
            Ok(dusk_capnp::dusk_capnp::value::Uint(u)) => u.to_string(),
            Ok(dusk_capnp::dusk_capnp::value::Bool(b)) => b.to_string(),
            Ok(dusk_capnp::dusk_capnp::value::List(_)) => "[list]".to_string(),
            Ok(dusk_capnp::dusk_capnp::value::Bytes(_)) => "[bytes]".to_string(),
            Ok(dusk_capnp::dusk_capnp::value::Fields(_)) => "[fields]".to_string(),
            Ok(dusk_capnp::dusk_capnp::value::Null(())) => "null".to_string(),
            Err(_) => "[error]".to_string(),
        }
    }
}

impl Server for DisplayStream {
    fn send(
        &mut self,
        params: dusk_capnp::dusk_capnp::stream::SendParams,
    ) -> Promise<(), capnp::Error> {
        let value = pry!(pry!(params.get()).get_value());
        print!("{}", Self::value_to_string(value));
        Promise::ok(())
    }

    fn done(
        &mut self,
        _: dusk_capnp::dusk_capnp::stream::DoneParams,
        _: dusk_capnp::dusk_capnp::stream::DoneResults,
    ) -> Promise<(), capnp::Error> {
        if let Some(done_sender) = self.done_sender.take() {
            pry!(done_sender
                .send(())
                .map_err(|_| capnp::Error::failed("failed to send done signal".to_string())));
            Promise::ok(())
        } else {
            Promise::err(capnp::Error::failed("done already called".to_string()))
        }
    }
}
