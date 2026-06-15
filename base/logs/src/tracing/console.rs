use super::collect::FieldValue;
use super::{HEX_ID_FIELDS, MESSAGE_FIELD};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use nu_ansi_term::{Color, Style};
use portable_atomic_util::Arc;
use std::io::Write as _;
use tracing::Level;

/// The level keyword's color, matching the `logs view` palette.
fn level_style(level: Level) -> Style {
    match level {
        Level::ERROR => Style::new().fg(Color::Red).bold(),
        Level::WARN => Style::new().fg(Color::Yellow).bold(),
        Level::INFO => Style::new().fg(Color::Green),
        Level::DEBUG => Style::new().fg(Color::Blue),
        Level::TRACE => Style::new().fg(Color::DarkGray),
    }
}

/// The message's color: red at ERROR, yellow at WARN, plain otherwise.
fn message_style(level: Level) -> Style {
    match level {
        Level::ERROR => Style::new().fg(Color::Red),
        Level::WARN => Style::new().fg(Color::Yellow),
        _ => Style::new(),
    }
}

/// Print one captured event to stdout, INFO and above: dim timestamp, colored
/// level, colored message, then the event's own fields and its span scope as
/// cyan `key=value` — ids ([`HEX_ID_FIELDS`]) as bare hex, so the console agrees
/// with the buffer and OTLP output. Lower-severity events are captured but not
/// printed.
pub(crate) fn print(
    metadata: &tracing::Metadata<'_>,
    event_fields: &[(&'static str, FieldValue)],
    scope: &[Arc<Vec<(&'static str, FieldValue)>>],
) {
    let level = *metadata.level();
    if level > Level::INFO {
        return;
    }

    let dim = Style::new().fg(Color::DarkGray);
    let timestamp = chrono::Local::now().format("%H:%M:%S%.3f").to_string();
    let mut line = dim.paint(timestamp).to_string();
    line.push(' ');
    line.push_str(
        &level_style(level)
            .paint(format!("{:<5}", level.as_str()))
            .to_string(),
    );

    if let Some((_, FieldValue::Text(message))) =
        event_fields.iter().find(|(name, _)| *name == MESSAGE_FIELD)
    {
        line.push(' ');
        line.push_str(&message_style(level).paint(message.as_str()).to_string());
    }

    // Event fields (sans message) then span scope, innermost-first, first value
    // per key — the same precedence the buffer's log record uses.
    let mut seen: Vec<&str> = Vec::new();
    let candidates = event_fields
        .iter()
        .filter(|(name, _)| *name != MESSAGE_FIELD)
        .chain(scope.iter().rev().flat_map(|fields| fields.iter()));
    for (name, value) in candidates {
        if seen.contains(name) {
            continue;
        }
        seen.push(name);
        line.push(' ');
        line.push_str(&Style::new().fg(Color::Cyan).paint(*name).to_string());
        line.push_str(&dim.paint("=").to_string());
        line.push_str(
            &Style::new()
                .fg(Color::LightYellow)
                .paint(field_value(name, value))
                .to_string(),
        );
    }

    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{line}");
}

fn field_value(name: &str, value: &FieldValue) -> String {
    // Ids ([`HEX_ID_FIELDS`]) render as bare hex — matching buffer enrichment —
    // whether captured as a 32-bit int or, when wider, a decimal string.
    if HEX_ID_FIELDS.contains(&name) {
        let id = match value {
            FieldValue::Int(integer) if *integer >= 0 => Some(*integer as u64),
            FieldValue::Text(text) => text.parse::<u64>().ok(),
            _ => None,
        };
        if let Some(id) = id {
            return format!("{id:x}");
        }
    }
    match value {
        FieldValue::Int(integer) => integer.to_string(),
        FieldValue::Double(double) => double.to_string(),
        FieldValue::Bool(boolean) => boolean.to_string(),
        FieldValue::Text(text) => text.clone(),
    }
}
