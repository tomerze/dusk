use std::collections::HashMap;

use anyhow::{anyhow, Result};
use base64::prelude::*;
use crossterm::style::Stylize;
use dusk_program::value::key_bytes_to_string;
use dusk_program::value::{Record, Value};
use nu_ansi_term::Color;
use nu_color_config::{ComputableStyle, StyleComputer};
use nu_protocol::engine::{EngineState, Stack};
use nu_protocol::{Config, Record as NuRecord, Signals, Span, TableMode, Value as NuValue};
use nu_table::{JustTable, TableOpts};

/// It's not a pretty job to convert Dusk values to beautiful terminal representations
/// this struct gets that job done.
/// And it does so by using Nushell's private API.
#[derive(Clone)]
pub struct DefaultDisplayEngine {
    markdown_skin: termimad::MadSkin,
    config: Config,
    signals: Signals,
    engine_state: EngineState,
    stack: Stack,
}

pub trait DisplayEngine {
    fn render_value(&self, value: Value) -> Result<String>;
    fn render_markdown(&self, markdown: &str) -> String;
}

impl DisplayEngine for DefaultDisplayEngine {
    fn render_value(&self, value: Value) -> Result<String> {
        self.value_to_string(value)
    }
    fn render_markdown(&self, markdown: &str) -> String {
        self.markdown_skin.inline(markdown).to_string()
    }
}

impl Default for DefaultDisplayEngine {
    fn default() -> Self {
        let mut markdown_skin = termimad::MadSkin::default();
        use termimad::crossterm::style::Color;
        markdown_skin.paragraph.set_fg(termimad::rgb(30, 30, 40));
        markdown_skin.bold.set_fg(Color::Grey);
        markdown_skin.headers[1].set_fg(Color::Yellow);
        markdown_skin.bullet.set_char('○');
        markdown_skin.bullet.set_fg(Color::DarkYellow);
        markdown_skin.inline_code.set_fg(Color::Cyan);
        markdown_skin.code_block.set_fg(Color::Cyan);

        Self::new(markdown_skin)
    }
}

impl DefaultDisplayEngine {
    pub fn new(markdown_skin: termimad::MadSkin) -> Self {
        let config = Config::default();
        let signals = Signals::empty();
        let engine_state = EngineState::new();
        let stack = Stack::new();

        DefaultDisplayEngine {
            markdown_skin,
            config,
            signals,
            engine_state,
            stack,
        }
    }

    fn convert_list_to_nu_list(&self, list: Vec<Value>) -> Result<Vec<NuValue>> {
        list.into_iter()
            .map(|value| self.convert_value(value))
            .collect()
    }

    fn convert_fields_to_nu_record(&self, fields: Record) -> Result<NuRecord> {
        let mut record = NuRecord::with_capacity(fields.fields.len());
        for (key, value) in fields.fields.iter() {
            let value = self.convert_value(value.clone())?;
            record.push(key_bytes_to_string(key.clone()), value);
        }
        Ok(record)
    }

    fn convert_value(&self, value: Value) -> Result<NuValue> {
        let span = Span::unknown();
        match value {
            Value::Null => Ok(NuValue::nothing(span)),
            Value::Uint(_)
            | Value::Text(_)
            | Value::Bytes(_)
            | Value::Bool(_)
            | Value::String(_) => Ok(NuValue::string(self.value_to_string(value)?, span)),
            Value::Record(fields) => {
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
            Value::String(s) => Ok(s),
            Value::Text(s) => Ok(self.render_markdown(&s)),
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
            Value::Record(fields) => {
                if let Some(rows) = self.fields_to_rows(&fields)? {
                    let table = JustTable::table(rows, self.table_opts(span, term_width))
                        .map_err(|err| anyhow!(err.to_string()))?;
                    return Ok(table.unwrap_or_default());
                }
                let nu_record = self.convert_fields_to_nu_record(fields)?;
                let table = JustTable::kv_table(nu_record, self.table_opts(span, term_width))
                    .map_err(|err| anyhow!(err.to_string()))?;
                Ok(table.unwrap_or_default())
            }
        }
    }

    fn table_opts(&self, span: Span, width: usize) -> TableOpts<'_> {
        let mut overrides: HashMap<String, ComputableStyle> = HashMap::new();
        overrides.insert(
            "header".into(),
            ComputableStyle::Static(Color::Yellow.bold()),
        );
        overrides.insert(
            "row_index".into(),
            ComputableStyle::Static(Color::Cyan.bold()),
        );
        let style = StyleComputer::new(&self.engine_state, &self.stack, overrides);
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

    fn fields_to_rows(&self, fields: &Record) -> Result<Option<Vec<NuValue>>> {
        if fields.fields.is_empty() {
            return Ok(None);
        }

        // Ensure all fields are lists check all rows are the same length and get that length
        let expected_len: usize = {
            let lengths_opt: Option<Vec<usize>> = fields
                .fields
                .iter()
                .map(|(_key, value)| match &value {
                    Value::List(l) => Some(l.len()),
                    _ => None,
                })
                .collect();

            let lengths = match lengths_opt {
                Some(l) => l,
                None => return Ok(None),
            };

            if !lengths.windows(2).all(|w| w[0] == w[1]) {
                return Ok(None);
            }

            lengths[0]
        };

        let row_count = expected_len;
        let mut rows = Vec::with_capacity(row_count);
        for index in 0..row_count {
            let mut record = NuRecord::with_capacity(fields.fields.len());
            for (key, value) in fields.fields.iter() {
                let list = match &value {
                    Value::List(list) => list,
                    _ => unreachable!(),
                };
                let cell = list.get(index).cloned().unwrap_or(Value::Null);
                let value = self.convert_value(cell)?;
                record.push(key_bytes_to_string(key.clone()), value);
            }
            rows.push(NuValue::record(record, Span::unknown()));
        }

        Ok(Some(rows))
    }
}
