use capnp::capability::Promise;
use std::print;
use std::string::ToString;

use dusk_capnp::{dusk_capnp::stream, pry};
use dusk_program::value::Value;

use tokio::sync::oneshot;

use colored_json::prelude::*;
use colored_json::{Color, Styler};

pub struct JsonStream {
    pub done_sender: Option<oneshot::Sender<()>>,
    pub colored: bool,
}

impl JsonStream {
    pub fn new_with_receiver(colored: bool) -> (Self, oneshot::Receiver<()>) {
        let (done_sender, done_receiver) = oneshot::channel();
        (
            JsonStream {
                done_sender: Some(done_sender),
                colored,
            },
            done_receiver,
        )
    }
}

impl stream::Server for JsonStream {
    fn send(
        &mut self,
        params: dusk_capnp::dusk_capnp::stream::SendParams,
    ) -> Promise<(), capnp::Error> {
        let value = pry!(pry!(params.get()).get_value());
        let mut json = pry!(
            pry!(Value::from_reader(value).map_err(|e| capnp::Error::failed(e.to_string())))
                .to_json_string()
                .map_err(|e| capnp::Error::failed(e.to_string()))
        );

        if self.colored {
            json = pry!(
                json.to_colored_json_with_styler(
                    ColorMode::default().eval(),
                    Styler {
                        key: Color::Green.bold(),
                        string_value: Color::Blue.bold(),
                        integer_value: Color::Cyan.bold(),
                        float_value: Color::Magenta.italic(),
                        object_brackets: Color::Yellow.bold(),
                        array_brackets: Color::Yellow.bold(),
                        ..Default::default()
                    }
                )
                .map_err(|e| capnp::Error::failed(e.to_string()))
            );
        }

        print!("{}\n\n", json); // Json objects are delimited by an empty line
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
                    .map_err(|_| capnp::Error::failed("failed to send done signal".to_string()))
            );
            Promise::ok(())
        } else {
            Promise::err(capnp::Error::failed("done already called".to_string()))
        }
    }
}
