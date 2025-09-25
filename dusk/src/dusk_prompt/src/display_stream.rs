use anyhow::Result;
use base64::prelude::*;
use capnp::capability::Promise;
use crossterm::style::Stylize;
use dusk_capnp::dusk_capnp::value;
use dusk_capnp::{dusk_capnp::stream::Server, pry};
use tokio::sync::oneshot;

pub struct DisplayStream {
    pub markdown_skin: termimad::MadSkin,
    pub done_sender: Option<oneshot::Sender<()>>,
}

impl DisplayStream {
    pub fn new_with_receiver(markdown_skin: &termimad::MadSkin) -> (Self, oneshot::Receiver<()>) {
        let (done_sender, done_receiver) = oneshot::channel();
        (
            DisplayStream {
                markdown_skin: markdown_skin.clone(),
                done_sender: Some(done_sender),
            },
            done_receiver,
        )
    }

    fn value_to_string(
        markdown_skin: &termimad::MadSkin,
        value: value::Reader<'_>,
    ) -> Result<String> {
        // TODO: fix this
        let which_value = value.which()?;
        let term_width = crossterm::terminal::size()?.0 as usize;
        match which_value {
            value::Text(Ok(reader)) => {
                let text = reader
                    .to_string()
                    .map_err(|e| anyhow::format_err!("failed to parse utf8 string: {}", e))?;
                Ok(markdown_skin.text(&text, Some(term_width)).to_string())
            }
            value::Int(i) => Ok(i.to_string().cyan().bold().to_string()),
            value::Uint(u) => Ok(u.to_string().cyan().bold().to_string()),
            value::Bool(b) => Ok(b.to_string().cyan().bold().to_string()),
            value::Bytes(Ok(b)) => Ok(markdown_skin
                .text(
                    &format!("`{}`", BASE64_STANDARD.encode(b)),
                    Some(term_width),
                )
                .to_string()),
            value::Null(()) => Ok("".to_string()),
            value::List(_) => Ok("[list]".to_string()),
            value::Fields(_) => Ok("[fields]".to_string()),
            _ => Err(anyhow::anyhow!("Couldn't display value")),
        }
    }
}

impl Server for DisplayStream {
    fn send(
        &mut self,
        params: dusk_capnp::dusk_capnp::stream::SendParams,
    ) -> Promise<(), capnp::Error> {
        let value = pry!(pry!(params.get()).get_value());
        print!(
            "{}",
            pry!(Self::value_to_string(&self.markdown_skin, value)
                .map_err(|e| capnp::Error::failed(e.to_string())))
        );
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
