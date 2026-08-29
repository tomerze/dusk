use capnp::capability::Promise;
use std::print;
use std::string::ToString;

use dusk_program::stream::{Stream, StreamMixin};
use dusk_program::value::Value;

use tokio::sync::oneshot;

use colored_json::prelude::*;
use colored_json::{Color, Styler};

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
        let mut json = match value.to_json_string() {
            Ok(json) => json,
            Err(error) => return Promise::err(capnp::Error::failed(error.to_string())),
        };

        if self.colored {
            json = match json.to_colored_json_with_styler(
                ColorMode::default().eval(),
                Styler {
                    key: Color::Green.bold(),
                    string_value: Color::Blue.bold(),
                    integer_value: Color::Cyan.bold(),
                    float_value: Color::Magenta.italic(),
                    object_brackets: Color::Yellow.bold(),
                    array_brackets: Color::Yellow.bold(),
                    ..Default::default()
                },
            ) {
                Ok(json) => json,
                Err(error) => return Promise::err(capnp::Error::failed(error.to_string())),
            };
        }

        print!("{}\n\n", json); // Json objects are delimited by an empty line
        Promise::ok(())
    }

    fn end(&mut self) {
        if let Some(done_sender) = self.done_sender.take() {
            let _ = done_sender.send(());
        }
    }
}
