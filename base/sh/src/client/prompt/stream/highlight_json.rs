use std::string::{String, ToString};

use nu_ansi_term::{Color, Style};
use serde_json::{Map, Value};

pub struct Styler {
    pub key: Style,
    pub string_value: Style,
    pub integer_value: Style,
    pub float_value: Style,
    pub bool_value: Style,
    pub nil_value: Style,
    pub object_brackets: Style,
    pub array_brackets: Style,
    pub object_colon: Style,
}

impl Default for Styler {
    fn default() -> Self {
        Styler {
            key: Color::Blue.bold(),
            string_value: Color::Green.normal(),
            integer_value: Style::new(),
            float_value: Style::new(),
            bool_value: Style::new(),
            nil_value: Color::DarkGray.bold(),
            object_brackets: Style::new().bold(),
            array_brackets: Style::new().bold(),
            object_colon: Style::new(),
        }
    }
}

const INDENT: &str = "  ";

pub fn highlight_json(value: &Value, styler: &Styler) -> serde_json::Result<String> {
    let mut output = String::new();
    write_value(&mut output, value, styler, 0)?;
    Ok(output)
}

fn write_styled(output: &mut String, style: Style, text: &str) {
    output.push_str(&style.paint(text).to_string());
}

fn write_indent(output: &mut String, depth: usize) {
    for _ in 0..depth {
        output.push_str(INDENT);
    }
}

fn write_value(
    output: &mut String,
    value: &Value,
    styler: &Styler,
    depth: usize,
) -> serde_json::Result<()> {
    match value {
        Value::Null => write_styled(output, styler.nil_value, "null"),
        Value::Bool(boolean) => write_styled(output, styler.bool_value, &boolean.to_string()),
        Value::Number(number) => {
            let style = if number.is_f64() {
                styler.float_value
            } else {
                styler.integer_value
            };
            write_styled(output, style, &number.to_string());
        }
        Value::String(string) => {
            write_styled(output, styler.string_value, &serde_json::to_string(string)?)
        }
        Value::Array(items) => write_array(output, items, styler, depth)?,
        Value::Object(members) => write_object(output, members, styler, depth)?,
    }
    Ok(())
}

fn write_array(
    output: &mut String,
    items: &[Value],
    styler: &Styler,
    depth: usize,
) -> serde_json::Result<()> {
    if items.is_empty() {
        write_styled(output, styler.array_brackets, "[]");
        return Ok(());
    }
    write_styled(output, styler.array_brackets, "[");
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push('\n');
        write_indent(output, depth + 1);
        write_value(output, item, styler, depth + 1)?;
    }
    output.push('\n');
    write_indent(output, depth);
    write_styled(output, styler.array_brackets, "]");
    Ok(())
}

fn write_object(
    output: &mut String,
    members: &Map<String, Value>,
    styler: &Styler,
    depth: usize,
) -> serde_json::Result<()> {
    if members.is_empty() {
        write_styled(output, styler.object_brackets, "{}");
        return Ok(());
    }
    write_styled(output, styler.object_brackets, "{");
    for (index, (key, member)) in members.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push('\n');
        write_indent(output, depth + 1);
        write_styled(output, styler.key, &serde_json::to_string(key)?);
        write_styled(output, styler.object_colon, ":");
        output.push(' ');
        write_value(output, member, styler, depth + 1)?;
    }
    output.push('\n');
    write_indent(output, depth);
    write_styled(output, styler.object_brackets, "}");
    Ok(())
}
