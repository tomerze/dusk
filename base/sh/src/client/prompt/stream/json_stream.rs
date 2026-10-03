use capnp::capability::Promise;
use std::print;
use std::string::ToString;

use dusk_program::stream::{Stream, StreamMixin};
use dusk_program::value::Value;

use tokio::sync::oneshot;

use nu_ansi_term::Color;

use super::highlight_json::{Styler, highlight_json};

pub struct JsonStream {
    pub done_sender: Option<oneshot::Sender<()>>,
    pub colored: bool,
}

impl JsonStream {
    pub fn new_with_receiver(colored: bool) -> (Stream<Self>, oneshot::Receiver<()>) {
        let (done_sender, done_receiver) = oneshot::channel();
        (
            Stream::new(JsonStream {
                done_sender: Some(done_sender),
                colored,
            }),
            done_receiver,
        )
    }
}

impl StreamMixin for JsonStream {
    fn send(&mut self, value: Value) -> Promise<(), capnp::Error> {
        let json = if self.colored {
            serde_json::to_value(&value).and_then(|json| {
                highlight_json(
                    &json,
                    &Styler {
                        key: Color::Green.bold(),
                        string_value: Color::Blue.bold(),
                        integer_value: Color::Cyan.bold(),
                        float_value: Color::Magenta.italic(),
                        object_brackets: Color::Yellow.bold(),
                        array_brackets: Color::Yellow.bold(),
                        ..Default::default()
                    },
                )
            })
        } else {
            value.to_json_string()
        };
        let json = match json {
            Ok(json) => json,
            Err(error) => return Promise::err(capnp::Error::failed(error.to_string())),
        };

        print!("{}\n\n", json); // Json objects are delimited by an empty line
        Promise::ok(())
    }

    fn end(&mut self) {
        if let Some(done_sender) = self.done_sender.take() {
            let _ = done_sender.send(());
        }
    }
}
