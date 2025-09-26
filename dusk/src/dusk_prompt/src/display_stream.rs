use anyhow::{anyhow, Result};
use base64::prelude::*;
use capnp::capability::Promise;
use crossterm::style::Stylize;
use dusk_capnp::value::Value;
use dusk_capnp::{dusk_capnp::stream::Server, pry};
use nu_color_config::StyleComputer;
use nu_protocol::engine::{EngineState, Stack};
use nu_protocol::{Config, Record as NuRecord, Signals, Span, TableMode, Value as NuValue};
use nu_table::{JustTable, TableOpts};
use std::collections::HashMap;
use tokio::sync::oneshot;

pub struct DisplayStream {
    pub markdown_skin: termimad::MadSkin,
    pub done_sender: Option<oneshot::Sender<()>>,
    config: Config,
    signals: Signals,
    engine_state: EngineState,
    stack: Stack,
}

impl DisplayStream {
    pub fn new_with_receiver(markdown_skin: &termimad::MadSkin) -> (Self, oneshot::Receiver<()>) {
        let (done_sender, done_receiver) = oneshot::channel();
        let config = Config::default();
        let signals = Signals::empty();
        let engine_state = EngineState::new();
        let stack = Stack::new();
        (
            DisplayStream {
                markdown_skin: markdown_skin.clone(),
                done_sender: Some(done_sender),
                config,
                signals,
                engine_state,
                stack,
            },
            done_receiver,
        )
    }

    fn convert_list_to_nu_list(&self, list: Vec<dusk_capnp::value::Value>) -> Result<Vec<NuValue>> {
        list.into_iter()
            .map(|value| self.convert_value(value))
            .collect()
    }

    fn convert_fields_to_nu_record(
        &self,
        fields: Vec<dusk_capnp::value::Field>,
    ) -> Result<NuRecord> {
        let mut record = NuRecord::with_capacity(fields.len());
        for field in fields {
            let value = self.convert_value(field.value)?;
            record.push(field.key, value);
        }
        Ok(record)
    }

    fn convert_value(&self, value: dusk_capnp::value::Value) -> Result<NuValue> {
        let span = Span::unknown();
        match value {
            Value::Null => Ok(NuValue::nothing(span)),
            Value::Uint(_) | Value::Text(_) | Value::Bytes(_) | Value::Bool(_) => {
                Ok(NuValue::string(self.value_to_string(value)?, span))
            }
            Value::Fields(fields) => {
                let record = self.convert_fields_to_nu_record(fields)?;
                Ok(NuValue::record(record, span))
            }
            Value::List(values) => {
                let values = self.convert_list_to_nu_list(values)?;
                Ok(NuValue::list(values, span))
            }
        }
    }

    fn value_to_string(&self, value: Value) -> Result<String> {
        let term_width = crossterm::terminal::size()?.0 as usize;
        let span = Span::unknown();

        match value {
            Value::Null => Ok("".to_string()),
            Value::Bool(b) => Ok(b.to_string().cyan().bold().to_string()),
            Value::Uint(u) => Ok(u.to_string().cyan().bold().to_string()),
            Value::Text(s) => Ok(self.markdown_skin.text(&s, Some(term_width)).to_string()),
            Value::Bytes(b) => Ok(self
                .markdown_skin
                .text(
                    &format!("`{}`", BASE64_STANDARD.encode(&b)),
                    Some(term_width),
                )
                .to_string()),
            Value::List(list) => {
                let nu_list = self.convert_list_to_nu_list(list)?;
                let table = JustTable::table(nu_list, self.table_opts(span, term_width))
                    .map_err(|err| anyhow!(err.to_string()))?;
                Ok(table.unwrap_or_default())
            }
            Value::Fields(fields) => {
                let nu_record = self.convert_fields_to_nu_record(fields)?;
                let table = JustTable::kv_table(nu_record, self.table_opts(span, term_width))
                    .map_err(|err| anyhow!(err.to_string()))?;
                Ok(table.unwrap_or_default())
            }
        }
    }

    fn table_opts(&self, span: Span, width: usize) -> TableOpts<'_> {
        let style =
            StyleComputer::new(&self.engine_state, &self.stack, HashMap::<String, _>::new());
        TableOpts::new(
            &self.config,
            style,
            &self.signals,
            span,
            width,
            TableMode::Rounded,
            0,
            false,
        )
    }
}

impl Server for DisplayStream {
    fn send(
        &mut self,
        params: dusk_capnp::dusk_capnp::stream::SendParams,
    ) -> Promise<(), capnp::Error> {
        let value = pry!(pry!(params.get()).get_value());
        let value =
            pry!(Value::from_reader(value).map_err(|e| capnp::Error::failed(e.to_string())));
        print!(
            "{}",
            pry!(self
                .value_to_string(value)
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
