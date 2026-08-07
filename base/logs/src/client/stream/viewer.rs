use std::io::{Write, stdout};
use std::path::PathBuf;
use std::string::String;
use std::time::Duration;
use std::vec::Vec;

use super::acknowledge;
use crate::common_capnp::any_value;
use crate::log_record_capnp::log_record;
use crate::logs_capnp::{logs_args, signal};
use base64::Engine as _;
use capnp::capability::Promise;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseEventKind,
};
use crossterm::terminal::{
    BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate, EnterAlternateScreen,
    LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute, queue};
use dusk_program::anyhow::Result;
use nu_ansi_term::{Color, Style};
use std::rc::Rc;
use tokio::sync::Notify;
use tokio::sync::mpsc::{self, Receiver};

/// Retained pager lines; the oldest are trimmed past this, mirroring the
/// node-side ring's drop-oldest nature.
const MAX_LINES: usize = 200_000;

/// Trim hysteresis: retention may overshoot [`MAX_LINES`] by up to this many
/// lines so the O(n) front-shift of a trim amortizes across appends.
const TRIM_CHUNK: usize = 4_096;

/// The hanging indent of continuation wrap-rows: the timestamp column
/// (`%H:%M:%S%.3f`, 12 chars), the level column (5 chars), and their
/// trailing spaces. Wrapped text aligns with the message, never under a
/// timestamp or level.
const WRAP_MARGIN: usize = 19;

/// A `LogsArgs.Stream` that pages the node's logs in an interactive
/// terminal viewer. Unlike the other streams it can't write on `send` — the
/// pager UI runs concurrently — so `send` feeds the pager's bounded channel
/// (the backpressure) and the pager drains it; `stop` is answered when the user
/// quits.
pub struct ViewerStream {
    entries: mpsc::Sender<Vec<Line>>,
    stop: Rc<Notify>,
    /// Fired on drop — i.e. when the node-side capability is gone — so the pager
    /// can surface the disconnect in its status bar even while paused.
    disconnected: Rc<Notify>,
}

impl ViewerStream {
    pub fn new() -> Self {
        // A few batches of slack between the stream and the pager; small, so a
        // paused pager parks the node instead of buffering a flood.
        let (entries, receiver) = mpsc::channel(16);
        let stop = Rc::new(Notify::new());
        let disconnected = Rc::new(Notify::new());
        let stop_for_pager = stop.clone();
        let disconnected_for_pager = disconnected.clone();
        tokio::task::spawn_local(async move {
            if let Err(error) = Pager::run(receiver, disconnected_for_pager).await {
                tracing::error!(error = %format!("{error:#}"), "log viewer failed");
            }
            // The pager quit (or failed): the node's cue to finish the stream.
            stop_for_pager.notify_one();
        });
        ViewerStream {
            entries,
            stop,
            disconnected,
        }
    }
}

impl Drop for ViewerStream {
    fn drop(&mut self) {
        // The node finished the stream or the connection died; wake the pager so
        // it marks the disconnect, regardless of whether it is following.
        self.disconnected.notify_one();
    }
}

impl Default for ViewerStream {
    fn default() -> Self {
        Self::new()
    }
}

impl logs_args::stream::Server for ViewerStream {
    fn send(&mut self, params: logs_args::stream::SendParams) -> Promise<(), capnp::Error> {
        let signal_batch = dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_signal_batch());
        let entries = dusk_capnp::pry!(signal_batch.get_signals());
        let ack = dusk_capnp::pry!(signal_batch.get_ack());
        // Render to pager lines here, off the capnp signals — the pager never
        // sees a log record, only its styled lines.
        let lines = entry_lines(entries);
        let sender = self.entries.clone();
        let stop = self.stop.clone();
        Promise::from_future(async move {
            // A closed channel means the pager already quit; signal stop so the
            // node finishes rather than dropping batches forever, and leave the
            // batch unacknowledged.
            if sender.send(lines).await.is_err() {
                stop.notify_one();
                return Ok(());
            }
            acknowledge(ack).await;
            Ok(())
        })
    }

    fn stop(
        &mut self,
        _params: logs_args::stream::StopParams,
        _results: logs_args::stream::StopResults,
    ) -> Promise<(), capnp::Error> {
        let stop = self.stop.clone();
        Promise::from_future(async move {
            stop.notified().await;
            Ok(())
        })
    }
}

/// The pager lines for a streamed batch, rendered straight off the capnp
/// signals. Spans aren't shown in the viewer (they go to the other streams);
/// only log records render. A signal that fails to read is skipped with a
/// warning — one bad signal must not drop the batch.
pub(crate) fn entry_lines(entries: capnp::struct_list::Reader<signal::Owned>) -> Vec<Line> {
    let mut lines = Vec::new();
    for entry in entries {
        match log_record_of(entry) {
            Ok(Some(log_record)) => {
                // The level is on the Signal envelope, not the log record.
                let severity_number = match entry.get_severity_number() {
                    Ok(severity) => severity as i32,
                    Err(capnp::NotInSchema(number)) => number as i32,
                };
                match log_record_lines(log_record, severity_number) {
                    Ok(log_record_lines) => lines.extend(log_record_lines),
                    Err(error) => tracing::warn!(%error, "skipping an unrenderable log record"),
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "skipping an unreadable signal"),
        }
    }
    lines
}

/// The log-record reader of a `Signal`, or `None` if it is a span — the viewer
/// shows only logs (spans go to the other sinks).
fn log_record_of(signal: signal::Reader) -> capnp::Result<Option<log_record::Reader>> {
    match signal.which()? {
        signal::Which::LogRecord(log_record) => Ok(Some(log_record?)),
        signal::Which::Span(_) => Ok(None),
    }
}

// ---- styled lines ----

#[derive(Clone)]
struct Span {
    style: Style,
    text: String,
}

#[derive(Clone)]
pub(crate) struct Line {
    spans: Vec<Span>,
    /// The concatenated span text — what search, selection, and yank act on.
    pub(crate) plain: String,
}

impl Line {
    fn new(spans: Vec<Span>) -> Self {
        let plain = spans.iter().map(|span| span.text.as_str()).collect();
        Line { spans, plain }
    }

    fn char_count(&self) -> usize {
        self.plain.chars().count()
    }
}

/// ASCII only, control characters stripped: everything the pager paints must
/// render on whatever terminal and locale the client has, and message content
/// can never smuggle escape sequences of its own.
fn sanitize(text: &str) -> String {
    text.replace('\t', "    ")
        .chars()
        .filter_map(|character| {
            if character.is_control() {
                None
            } else if character.is_ascii() {
                Some(character)
            } else {
                Some('?')
            }
        })
        .collect()
}

/// The text label for an OTLP severity number, by group. `None` for unspecified
/// (0) or an unknown number.
fn severity_text(severity_number: i32) -> Option<&'static str> {
    match severity_number {
        1..=4 => Some("TRACE"),
        5..=8 => Some("DEBUG"),
        9..=12 => Some("INFO"),
        13..=16 => Some("WARN"),
        17..=20 => Some("ERROR"),
        21..=24 => Some("FATAL"),
        _ => None,
    }
}

fn level_style(severity: &str) -> Style {
    match severity {
        "ERROR" => Style::new().fg(Color::Red).bold(),
        "WARN" => Style::new().fg(Color::Yellow).bold(),
        "INFO" => Style::new().fg(Color::Green),
        "DEBUG" => Style::new().fg(Color::Blue),
        _ => Style::new().fg(Color::DarkGray),
    }
}

fn message_style(severity: &str) -> Style {
    match severity {
        "ERROR" => Style::new().fg(Color::Red),
        "WARN" => Style::new().fg(Color::Yellow),
        _ => Style::new(),
    }
}

fn any_value_text(value: any_value::Reader) -> capnp::Result<String> {
    use any_value::Which;
    Ok(match value.which()? {
        Which::StringValue(text) => sanitize(text?.to_str()?),
        Which::BoolValue(boolean) => boolean.to_string(),
        Which::IntValue(integer) => integer.to_string(),
        Which::DoubleValue(double) => double.to_string(),
        Which::BytesValue(bytes) => format!("({} bytes)", bytes?.len()),
        Which::ArrayValue(_) => "[...]".to_string(),
        Which::KvlistValue(_) => "{...}".to_string(),
    })
}

/// One log record → its pager lines (continuations for multi-line messages),
/// rich-style: dim time, colored level, message, cyan `key=values`.
fn log_record_lines(
    log_record: log_record::Reader,
    severity_number: i32,
) -> capnp::Result<Vec<Line>> {
    let severity = severity_text(severity_number).unwrap_or("");
    let time = chrono::DateTime::from_timestamp_nanos(log_record.get_time_unix_nano() as i64)
        .with_timezone(&chrono::Local)
        .format("%H:%M:%S%.3f")
        .to_string();

    let message = if log_record.has_body() {
        any_value_text(log_record.get_body()?)?
    } else {
        String::new()
    };

    let mut fields: Vec<(String, String)> = Vec::new();
    for attribute in log_record.get_attributes()? {
        let key = attribute.get_key()?.to_str()?;
        let value = if attribute.has_value() {
            any_value_text(attribute.get_value()?)?
        } else {
            String::new()
        };
        fields.push((key.to_string(), value));
    }

    let dim = Style::new().fg(Color::DarkGray);
    let mut message_parts = message.split('\n');
    let first_message = message_parts.next().unwrap_or("").to_string();

    let mut spans = vec![
        Span {
            style: dim,
            text: time,
        },
        Span {
            style: Style::new(),
            text: " ".into(),
        },
        Span {
            style: level_style(severity),
            text: format!("{severity:<5}"),
        },
        Span {
            style: Style::new(),
            text: " ".into(),
        },
        Span {
            style: message_style(severity),
            text: first_message,
        },
    ];
    for (key, value) in &fields {
        spans.push(Span {
            style: Style::new(),
            text: " ".into(),
        });
        spans.push(Span {
            style: Style::new().fg(Color::Cyan),
            text: key.clone(),
        });
        spans.push(Span {
            style: dim,
            text: "=".into(),
        });
        spans.push(Span {
            style: Style::new().fg(Color::LightYellow),
            text: value.clone(),
        });
    }
    let mut lines = vec![Line::new(spans)];
    for continuation in message_parts {
        lines.push(Line::new(vec![
            Span {
                style: dim,
                text: "             | ".into(),
            },
            Span {
                style: message_style(severity),
                text: continuation.to_string(),
            },
        ]));
    }
    Ok(lines)
}

// ---- the pager ----

#[derive(Clone, Copy, PartialEq)]
enum VisualKind {
    /// `v` — charwise.
    Char,
    /// `V` — linewise.
    Line,
    /// `Ctrl-V` — blockwise: the rectangle spanned by anchor and cursor.
    Block,
}

enum Mode {
    Normal,
    /// A visual selection anchored at `(line, column)`.
    Visual {
        anchor: (usize, usize),
        kind: VisualKind,
    },
    /// `/`, `?`, or `:` input being typed.
    Input {
        prefix: char,
        buffer: String,
    },
}

enum Outcome {
    Continue,
    Quit,
}

/// An ordered visual selection: `start <= end` position-wise, both
/// `(line, column)`. For [`VisualKind::Block`] the columns of `start` and
/// `end` are independent — use [`Selection::columns`] for the rectangle.
#[derive(Clone, Copy)]
struct Selection {
    start: (usize, usize),
    end: (usize, usize),
    kind: VisualKind,
}

impl Selection {
    /// The block rectangle's inclusive column range.
    fn columns(&self) -> (usize, usize) {
        (self.start.1.min(self.end.1), self.start.1.max(self.end.1))
    }

    /// The selected half-open column span on line `index` (clamped to
    /// `length`), per kind — the one place yank and rendering agree on what
    /// is selected.
    fn span_on_line(&self, index: usize, length: usize) -> (usize, usize) {
        match self.kind {
            VisualKind::Line => (0, length),
            VisualKind::Char => (
                if index > self.start.0 {
                    0
                } else {
                    self.start.1.min(length)
                },
                if index < self.end.0 {
                    length
                } else {
                    (self.end.1 + 1).min(length)
                },
            ),
            VisualKind::Block => {
                let (low, high) = self.columns();
                (low.min(length), (high + 1).min(length))
            }
        }
    }
}

struct Pager {
    lines: Vec<Line>,
    cursor: (usize, usize),
    /// The first visual row on screen: a line index plus a wrap-row offset
    /// within that line — lines wrap to the terminal width, so one line can
    /// occupy several rows.
    top: (usize, usize),
    follow: bool,
    mode: Mode,
    pattern: Option<String>,
    search_backward: bool,
    pending: Option<char>,
    flash: Option<String>,
    width: usize,
    height: usize,
}

/// Restores the terminal even on error or panic.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(
            stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            cursor::SetCursorStyle::SteadyBlock
        )?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            stdout(),
            DisableMouseCapture,
            LeaveAlternateScreen,
            cursor::SetCursorStyle::DefaultUserShape,
            cursor::Show
        );
        let _ = disable_raw_mode();
    }
}

fn spawn_event_thread() -> tokio::sync::mpsc::UnboundedReceiver<Event> {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        loop {
            if sender.is_closed() {
                return;
            }
            match crossterm::event::poll(Duration::from_millis(100)) {
                Ok(true) => {
                    let Ok(event) = crossterm::event::read() else {
                        return;
                    };
                    if sender.send(event).is_err() {
                        return;
                    }
                }
                Ok(false) => {}
                Err(_) => return,
            }
        }
    });
    receiver
}

impl Pager {
    async fn run(mut entries: Receiver<Vec<Line>>, disconnected: Rc<Notify>) -> Result<()> {
        let (width, height) = crossterm::terminal::size()?;
        let mut pager = Pager {
            lines: Vec::new(),
            cursor: (0, 0),
            top: (0, 0),
            follow: true,
            mode: Mode::Normal,
            pattern: None,
            search_backward: false,
            pending: None,
            flash: None,
            width: width as usize,
            height: height as usize,
        };

        let guard = TerminalGuard::enter()?;
        let mut events = spawn_event_thread();

        pager.draw()?;
        let mut disconnected_exit = false;
        'pager: loop {
            tokio::select! {
                // Intake is gated on FOLLOW: in NORMAL mode nothing is received,
                // so nothing can move — not even retention trimming — and the
                // stream's flow-control window fills until the node parks.
                // Resuming drains the backlog.
                batch = entries.recv(), if pager.follow => match batch {
                    Some(lines) => pager.append(lines),
                    // The node side is gone; nothing more will ever arrive.
                    None => {
                        disconnected_exit = true;
                        break 'pager;
                    }
                },
                _ = disconnected.notified() => {
                    disconnected_exit = true;
                    break 'pager;
                }
                event = events.recv() => {
                    let Some(event) = event else { break };
                    if matches!(pager.handle(event)?, Outcome::Quit) {
                        break;
                    }
                }
            }
            // Drain whatever is already queued before paying for a frame —
            // under a log flood this folds many batches into one redraw.
            while pager.follow {
                match entries.try_recv() {
                    Ok(lines) => pager.append(lines),
                    Err(_) => break,
                }
            }
            while let Ok(event) = events.try_recv() {
                if matches!(pager.handle(event)?, Outcome::Quit) {
                    break 'pager;
                }
            }
            pager.draw()?;
        }

        drop(events);
        drop(guard);

        if disconnected_exit {
            match pager.write_buffer(None) {
                Ok(path) => tracing::warn!(
                    path = %path.display(),
                    "logs view connection lost, no worries the logs you viewed were saved to a file"
                ),
                Err(error) => tracing::error!(
                    %error,
                    "logs view connection lost, and there was an error while attempting to save the viewed logs to a file"
                ),
            }
        }
        Ok(())
    }

    fn append(&mut self, lines: Vec<Line>) {
        self.lines.extend(lines);
        // Trimming shifts the whole vector forward, so it runs once per
        // TRIM_CHUNK of overshoot rather than on every batch at the cap.
        if self.lines.len() > MAX_LINES + TRIM_CHUNK {
            let trim = self.lines.len() - MAX_LINES;
            self.lines.drain(..trim);
            self.cursor.0 = self.cursor.0.saturating_sub(trim);
            if self.top.0 >= trim {
                self.top.0 -= trim;
            } else {
                self.top = (0, 0);
            }
            if let Mode::Visual { anchor, .. } = &mut self.mode {
                anchor.0 = anchor.0.saturating_sub(trim);
            }
        }
        if self.follow {
            self.cursor = (self.lines.len().saturating_sub(1), 0);
            self.scroll_to_cursor();
        }
    }

    fn viewport_rows(&self) -> usize {
        self.height.saturating_sub(1).max(1)
    }

    /// Chars a continuation wrap-row holds: the width minus the hanging
    /// indent, except on terminals too narrow for the indent to fit.
    fn continuation_width(&self) -> usize {
        let width = self.width.max(1);
        if width > WRAP_MARGIN {
            width - WRAP_MARGIN
        } else {
            width
        }
    }

    /// The wrap-row and screen column of char-column `column` under the
    /// hanging indent: the first wrap-row spans the full width, every
    /// continuation row starts [`WRAP_MARGIN`] cells in.
    fn wrap_position(&self, column: usize) -> (usize, usize) {
        let width = self.width.max(1);
        if column < width {
            return (0, column);
        }
        let continuation = self.continuation_width();
        let margin = width - continuation;
        (
            1 + (column - width) / continuation,
            margin + (column - width) % continuation,
        )
    }

    /// Wrap-rows line `index` occupies at the current width.
    fn rows_of(&self, index: usize) -> usize {
        let width = self.width.max(1);
        self.lines
            .get(index)
            .map(|line| {
                let chars = line.char_count();
                if chars <= width {
                    1
                } else {
                    1 + (chars - width).div_ceil(self.continuation_width())
                }
            })
            .unwrap_or(1)
    }

    /// The visual position `rows` wrap-rows above `from`, clamped to the
    /// start.
    fn back_up(&self, from: (usize, usize), rows: usize) -> (usize, usize) {
        let (mut line, mut offset) = from;
        let mut remaining = rows;
        while remaining > 0 {
            if offset >= remaining {
                offset -= remaining;
                break;
            }
            remaining -= offset + 1;
            if line == 0 {
                return (0, 0);
            }
            line -= 1;
            offset = self.rows_of(line) - 1;
        }
        (line, offset)
    }

    /// Wrap-rows from visual position `top` down to `target`; `None` when
    /// `target` is above `top`.
    fn rows_between(&self, top: (usize, usize), target: (usize, usize)) -> Option<usize> {
        if target < top {
            return None;
        }
        let mut rows = 0;
        let mut line = top.0;
        let mut offset = top.1;
        while line < target.0 {
            rows += self.rows_of(line) - offset;
            line += 1;
            offset = 0;
        }
        Some(rows + target.1 - offset)
    }

    fn scroll_to_cursor(&mut self) {
        let rows = self.viewport_rows();
        // While following, the anchor is the cursor line's *last* wrap-row,
        // so a tail that wraps stays fully on screen.
        let target = if self.follow {
            (self.cursor.0, self.rows_of(self.cursor.0) - 1)
        } else {
            (self.cursor.0, self.wrap_position(self.cursor.1).0)
        };
        if target < self.top {
            self.top = target;
            return;
        }
        // Every line is at least one wrap-row, so a cursor more than a
        // viewport of *lines* below the top is past the bottom without
        // walking the rows in between.
        let distance = if self.cursor.0 - self.top.0 >= rows {
            rows
        } else {
            self.rows_between(self.top, target).unwrap_or(0)
        };
        if distance >= rows {
            self.top = self.back_up(target, rows - 1);
        }
    }

    fn clamp_column(&mut self) {
        let limit = self
            .lines
            .get(self.cursor.0)
            .map(|line| line.char_count().saturating_sub(1))
            .unwrap_or(0);
        self.cursor.1 = self.cursor.1.min(limit);
    }

    fn move_cursor_line(&mut self, delta: isize) {
        if self.lines.is_empty() {
            return;
        }
        let last = self.lines.len() - 1;
        let target = self.cursor.0.saturating_add_signed(delta).min(last);
        if delta < 0 && target < self.cursor.0 {
            self.follow = false;
        }
        self.cursor.0 = target;
        // Moving down never re-enters FOLLOW, even onto the last line — the
        // modes are explicit: `f` / `G` (or `:N` to the bottom) follow.
        self.clamp_column();
        self.scroll_to_cursor();
    }

    fn jump_bottom(&mut self) {
        if self.lines.is_empty() {
            return;
        }
        self.cursor = (self.lines.len() - 1, 0);
        self.follow = true;
        self.scroll_to_cursor();
    }

    /// vi-style `:N` (0-based here): follow resumes only when N is the
    /// bottom.
    fn jump_to_line(&mut self, index: usize) {
        if self.lines.is_empty() {
            return;
        }
        let last = self.lines.len() - 1;
        self.cursor = (index.min(last), 0);
        self.follow = self.cursor.0 == last;
        self.scroll_to_cursor();
    }

    fn find_from(
        &self,
        from: (usize, usize),
        backward: bool,
        include_start: bool,
    ) -> Option<(usize, usize)> {
        let pattern = self.pattern.as_deref()?;
        if pattern.is_empty() || self.lines.is_empty() {
            return None;
        }
        // The search domain is what exists when the search starts: appends
        // landing mid-search don't extend it, so a pattern matching every
        // line of a live flood still terminates after one wrap-around.
        let count = self.lines.len();
        let mut line_index = from.0.min(count - 1);
        for step in 0..=count {
            let line = &self.lines[line_index];
            let chars: Vec<char> = line.plain.chars().collect();
            let columns: Vec<usize> = matches_in(&chars, pattern);
            let candidate = if backward {
                columns
                    .iter()
                    .rev()
                    .find(|&&column| {
                        if step == 0 {
                            if include_start {
                                column <= from.1
                            } else {
                                column < from.1
                            }
                        } else {
                            true
                        }
                    })
                    .copied()
            } else {
                columns
                    .iter()
                    .find(|&&column| {
                        if step == 0 {
                            if include_start {
                                column >= from.1
                            } else {
                                column > from.1
                            }
                        } else {
                            true
                        }
                    })
                    .copied()
            };
            if let Some(column) = candidate {
                return Some((line_index, column));
            }
            line_index = if backward {
                (line_index + count - 1) % count
            } else {
                (line_index + 1) % count
            };
        }
        None
    }

    fn search(&mut self, backward: bool, include_start: bool) {
        match self.find_from(self.cursor, backward, include_start) {
            Some(found) => {
                self.cursor = found;
                self.follow = false;
                self.scroll_to_cursor();
            }
            None => {
                let pattern = self.pattern.clone().unwrap_or_default();
                self.flash = Some(format!("Pattern not found: {pattern}"));
            }
        }
    }

    // ---- yank ----

    fn selection(&self) -> Option<Selection> {
        let Mode::Visual { anchor, kind } = &self.mode else {
            return None;
        };
        let (start, end) = if *anchor <= self.cursor {
            (*anchor, self.cursor)
        } else {
            (self.cursor, *anchor)
        };
        Some(Selection {
            start,
            end,
            kind: *kind,
        })
    }

    fn yank_text(&self, selection: &Selection) -> String {
        let Selection { start, end, kind } = *selection;
        let mut parts = Vec::new();
        for index in start.0..=end.0.min(self.lines.len().saturating_sub(1)) {
            let plain = &self.lines[index].plain;
            if kind == VisualKind::Line {
                parts.push(plain.clone());
                continue;
            }
            let chars: Vec<char> = plain.chars().collect();
            let (from, to) = selection.span_on_line(index, chars.len());
            parts.push(chars[from..to].iter().collect());
        }
        let mut text: String = parts.join("\n");
        if kind == VisualKind::Line {
            text.push('\n');
        }
        text
    }

    /// The selection if one is active (leaving visual mode), the cursor's
    /// line otherwise — into the clipboard.
    fn copy_selection_or_line(&mut self) -> Result<()> {
        if let Some(selection) = self.selection() {
            let text = self.yank_text(&selection);
            self.copy_to_clipboard(&text)?;
            self.flash = Some(copy_summary(&selection));
            self.mode = Mode::Normal;
        } else if let Some(line) = self.lines.get(self.cursor.0) {
            let text = format!("{}\n", line.plain);
            self.copy_to_clipboard(&text)?;
            self.flash = Some("copied 1 line to clipboard".into());
        }
        Ok(())
    }

    fn copy_to_clipboard(&mut self, text: &str) -> Result<()> {
        // A native clipboard tool first — not every terminal honors OSC 52
        // (VTE-based ones ignore it entirely). OSC 52 stays as the fallback:
        // the terminal owns the clipboard there, so it works over SSH.
        if copy_with_system_tool(text) {
            return Ok(());
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(text);
        let mut out = stdout();
        write!(out, "\x1b]52;c;{encoded}\x07")?;
        out.flush()?;
        Ok(())
    }

    fn write_buffer(&self, path: Option<&str>) -> std::io::Result<PathBuf> {
        let path = match path {
            Some(path) => PathBuf::from(path),
            None => PathBuf::from(format!(
                "/tmp/dusk-logs-{}.log",
                chrono::Local::now().format("%Y%m%d-%H%M%S")
            )),
        };
        let mut file = std::fs::File::create(&path)?;
        for line in &self.lines {
            writeln!(file, "{}", line.plain)?;
        }
        file.flush()?;
        Ok(path)
    }

    /// Write the buffer for an interactive `:w` / `s` and report where it
    /// landed (or why it failed) in the status bar.
    fn write_and_flash(&mut self, path: Option<&str>) {
        self.flash = Some(match self.write_buffer(path) {
            Ok(path) => format!("wrote {} lines to {}", self.lines.len(), path.display()),
            Err(error) => format!("write failed: {error}"),
        });
    }

    // ---- input ----

    fn handle(&mut self, event: Event) -> Result<Outcome> {
        match event {
            Event::Key(key) => return self.handle_key(key),
            Event::Resize(width, height) => {
                self.width = width as usize;
                self.height = height as usize;
                self.scroll_to_cursor();
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.follow = false;
                    self.move_cursor_line(-3);
                }
                MouseEventKind::ScrollDown => self.move_cursor_line(3),
                _ => {}
            },
            _ => {}
        }
        Ok(Outcome::Continue)
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Outcome> {
        // Kitty-protocol terminals also deliver Release events; acting on
        // them would double every keypress (`gg` from a single `g`).
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return Ok(Outcome::Continue);
        }
        self.flash = None;

        if let Mode::Input { prefix, buffer } = &mut self.mode {
            match key.code {
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Backspace => {
                    if buffer.pop().is_none() {
                        self.mode = Mode::Normal;
                    }
                }
                KeyCode::Enter => {
                    let prefix = *prefix;
                    let entered = std::mem::take(buffer);
                    self.mode = Mode::Normal;
                    match prefix {
                        ':' => {
                            let trimmed = entered.trim();
                            if matches!(trimmed, "q" | "q!" | "quit") {
                                return Ok(Outcome::Quit);
                            } else if trimmed == "w" {
                                self.write_and_flash(None);
                            } else if let Some(path) = trimmed.strip_prefix("w ") {
                                self.write_and_flash(Some(path.trim()));
                            } else if let Ok(number) = trimmed.parse::<usize>() {
                                self.jump_to_line(number.saturating_sub(1));
                            } else {
                                self.flash = Some(format!("Not a viewer command: {trimmed}"));
                            }
                        }
                        slash => {
                            if !entered.is_empty() {
                                self.pattern = Some(entered);
                                self.search_backward = slash == '?';
                            }
                            let backward = self.search_backward;
                            self.search(backward, false);
                        }
                    }
                }
                KeyCode::Char(character) => buffer.push(character),
                _ => {}
            }
            return Ok(Outcome::Continue);
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            let rows = self.viewport_rows() as isize;
            match key.code {
                // Ctrl+Shift+C copies, like `y`; bare Ctrl+C quits. Whether
                // the former reaches us at all is the terminal's call — many
                // keep it for their own copy shortcut.
                KeyCode::Char('c') | KeyCode::Char('C')
                    if key.modifiers.contains(KeyModifiers::SHIFT) =>
                {
                    self.copy_selection_or_line()?;
                }
                KeyCode::Char('c') => return Ok(Outcome::Quit),
                KeyCode::Char('d') => self.move_cursor_line(rows / 2),
                KeyCode::Char('u') => self.move_cursor_line(-(rows / 2)),
                KeyCode::Char('f') => self.move_cursor_line(rows),
                KeyCode::Char('b') => self.move_cursor_line(-rows),
                KeyCode::Char('v') => self.toggle_visual(VisualKind::Block),
                _ => {}
            }
            self.pending = None;
            return Ok(Outcome::Continue);
        }

        let pending = self.pending.take();
        match key.code {
            KeyCode::Char('q') => return Ok(Outcome::Quit),
            KeyCode::Char('s') => self.write_and_flash(None),
            KeyCode::Char(':') => {
                self.mode = Mode::Input {
                    prefix: ':',
                    buffer: String::new(),
                }
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Input {
                    prefix: '/',
                    buffer: String::new(),
                }
            }
            KeyCode::Char('?') => {
                self.mode = Mode::Input {
                    prefix: '?',
                    buffer: String::new(),
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor_line(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor_line(-1),
            KeyCode::Char('h') | KeyCode::Left => {
                self.cursor.1 = self.cursor.1.saturating_sub(1);
                self.scroll_to_cursor();
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.cursor.1 += 1;
                self.clamp_column();
                self.scroll_to_cursor();
            }
            KeyCode::Char('0') | KeyCode::Home => {
                self.cursor.1 = 0;
                self.scroll_to_cursor();
            }
            KeyCode::Char('$') | KeyCode::End => {
                self.cursor.1 = usize::MAX;
                self.clamp_column();
                self.scroll_to_cursor();
            }
            KeyCode::PageDown => self.move_cursor_line(self.viewport_rows() as isize),
            KeyCode::PageUp => self.move_cursor_line(-(self.viewport_rows() as isize)),
            KeyCode::Char('g') => {
                if pending == Some('g') {
                    self.cursor = (0, 0);
                    self.follow = false;
                    self.scroll_to_cursor();
                } else {
                    self.pending = Some('g');
                }
            }
            KeyCode::Char('G') | KeyCode::Char('f') | KeyCode::Char('F') => self.jump_bottom(),
            KeyCode::Char('n') => {
                let backward = self.search_backward;
                self.search(backward, false);
            }
            KeyCode::Char('N') => {
                let backward = !self.search_backward;
                self.search(backward, false);
            }
            KeyCode::Char('v') => self.toggle_visual(VisualKind::Char),
            KeyCode::Char('V') => self.toggle_visual(VisualKind::Line),
            KeyCode::Char('y') => {
                if self.selection().is_some() || pending == Some('y') {
                    self.copy_selection_or_line()?;
                } else {
                    self.pending = Some('y');
                }
            }
            // Out of SEARCH mode: drop the pattern, and the highlights with it.
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.pattern = None;
            }
            _ => {}
        }
        Ok(Outcome::Continue)
    }

    fn toggle_visual(&mut self, kind: VisualKind) {
        self.mode = match &self.mode {
            Mode::Visual { kind: current, .. } if *current == kind => Mode::Normal,
            // Switching kinds keeps the anchor, like vim.
            Mode::Visual { anchor, .. } => Mode::Visual {
                anchor: *anchor,
                kind,
            },
            _ => {
                // A live tail would drag the selection endpoint along.
                self.follow = false;
                Mode::Visual {
                    anchor: self.cursor,
                    kind,
                }
            }
        };
    }

    // ---- rendering ----

    /// One frame: synchronized (the terminal applies it atomically), each row
    /// overwritten in place and cleared to end-of-line *after* printing —
    /// clearing first is what makes a repaint flicker.
    fn draw(&self) -> Result<()> {
        let mut out = stdout();
        queue!(out, BeginSynchronizedUpdate)?;
        let rows = self.viewport_rows();
        let mut position = self.top;
        for row in 0..rows {
            queue!(out, cursor::MoveTo(0, row as u16))?;
            if let Some(line) = self.lines.get(position.0) {
                queue!(
                    out,
                    crossterm::style::Print(self.render_row(position.0, line, position.1))
                )?;
                position = if position.1 + 1 < self.rows_of(position.0) {
                    (position.0, position.1 + 1)
                } else {
                    (position.0 + 1, 0)
                };
            } else if self.lines.is_empty() && row == 0 {
                queue!(
                    out,
                    crossterm::style::Print(
                        Style::new()
                            .fg(Color::DarkGray)
                            .paint("waiting for logs...")
                            .to_string()
                    )
                )?;
            }
            queue!(out, Clear(ClearType::UntilNewLine))?;
        }
        queue!(out, cursor::MoveTo(0, rows as u16))?;
        queue!(out, crossterm::style::Print(self.render_status()))?;
        queue!(out, Clear(ClearType::UntilNewLine))?;
        // The real terminal cursor, vim-style: a block on the cell in normal
        // and visual mode, a bar at the input position while typing.
        let column_limit = self.width.saturating_sub(1);
        match &self.mode {
            Mode::Input { buffer, .. } => {
                let column = (1 + buffer.chars().count()).min(column_limit);
                queue!(
                    out,
                    cursor::MoveTo(column as u16, rows as u16),
                    cursor::SetCursorStyle::SteadyBar
                )?;
            }
            _ => {
                let (wrap_row, screen_column) = self.wrap_position(self.cursor.1);
                let row = self
                    .rows_between(self.top, (self.cursor.0, wrap_row))
                    .unwrap_or(0)
                    .min(rows.saturating_sub(1));
                let column = screen_column.min(column_limit);
                queue!(
                    out,
                    cursor::MoveTo(column as u16, row as u16),
                    cursor::SetCursorStyle::SteadyBlock
                )?;
            }
        }
        queue!(out, EndSynchronizedUpdate)?;
        out.flush()?;
        Ok(())
    }

    fn render_status(&self) -> String {
        let (left, left_style) = match &self.mode {
            Mode::Input { prefix, buffer } => (format!("{prefix}{buffer}"), Style::new().bold()),
            Mode::Visual { .. } => match &self.flash {
                Some(flash) => (flash.clone(), Style::new().bold()),
                None => (String::new(), Style::new()),
            },
            Mode::Normal => match &self.flash {
                Some(flash) => (flash.clone(), Style::new().bold()),
                None if !self.follow => (
                    "f to follow · q to quit · s to save · h, j, k, l to navigate".to_string(),
                    Style::new().fg(Color::DarkGray),
                ),
                None => (
                    "q to quit · s to save · h, j, k, l to navigate".to_string(),
                    Style::new().fg(Color::DarkGray),
                ),
            },
        };
        let position = if self.lines.is_empty() {
            "0/0".to_string()
        } else {
            format!("{}/{}", self.cursor.0 + 1, self.lines.len())
        };
        // The mode, rightmost: the visual kinds; SEARCH while a pattern is
        // being typed or is active (Esc drops it); FOLLOW while tailing;
        // NORMAL otherwise.
        let searching = matches!(
            &self.mode,
            Mode::Input { prefix: '/', .. } | Mode::Input { prefix: '?', .. }
        ) || self.pattern.is_some();
        let mode_chip = match &self.mode {
            Mode::Visual {
                kind: VisualKind::Char,
                ..
            } => "  VISUAL",
            Mode::Visual {
                kind: VisualKind::Line,
                ..
            } => "  VISUAL LINE",
            Mode::Visual {
                kind: VisualKind::Block,
                ..
            } => "  VISUAL BLOCK",
            _ if searching && !self.follow => "  SEARCH",
            _ if self.follow => "  FOLLOW",
            _ => "  NORMAL",
        };
        let match_counter = match self.pattern.as_deref() {
            Some(pattern) if !pattern.is_empty() => {
                let mut total = 0usize;
                let mut through = 0usize;
                for (index, line) in self.lines.iter().enumerate() {
                    if line.plain.contains(pattern) {
                        total += 1;
                        if index <= self.cursor.0 {
                            through += 1;
                        }
                    }
                }
                format!("{through}/{total} matches · ")
            }
            _ => String::new(),
        };
        let chip_style = Style::new().fg(Color::Cyan).bold();
        let right_plain = format!("{match_counter}{position}{mode_chip}");
        let left: String = left
            .chars()
            .take(self.width.saturating_sub(right_plain.chars().count() + 1))
            .collect();
        let padding = self
            .width
            .saturating_sub(left.chars().count() + right_plain.chars().count());
        format!(
            "{}{}{}{}{}",
            left_style.paint(&left),
            " ".repeat(padding),
            Style::new().fg(Color::DarkGray).paint(match_counter),
            Style::new().fg(Color::DarkGray).paint(position),
            chip_style.paint(mode_chip),
        )
    }

    /// Compose one wrap-row of line `index` (`wrap_row` rows into the line):
    /// base span styles, search-match and selection overlays.
    fn render_row(&self, index: usize, line: &Line, wrap_row: usize) -> String {
        let chars: Vec<(char, Style)> = line
            .spans
            .iter()
            .flat_map(|span| {
                span.text
                    .chars()
                    .map(move |character| (character, span.style))
            })
            .collect();

        let mut overlays: Vec<Option<Style>> = vec![None; chars.len()];

        if let Some(pattern) = self.pattern.as_deref() {
            let plain_chars: Vec<char> = line.plain.chars().collect();
            let pattern_length = pattern.chars().count();
            for start in matches_in(&plain_chars, pattern) {
                for offset in 0..pattern_length {
                    if let Some(slot) = overlays.get_mut(start + offset) {
                        *slot = Some(Style::new().fg(Color::Black).on(Color::Yellow));
                    }
                }
            }
        }

        if let Some(selection) = self.selection()
            && index >= selection.start.0
            && index <= selection.end.0
        {
            let (from, to) = selection.span_on_line(index, chars.len());
            for slot in overlays.iter_mut().take(to).skip(from) {
                *slot = Some(Style::new().fg(Color::Black).on(Color::LightBlue));
            }
        }

        // Paint runs of identical style, not characters: a frame repaints the
        // whole viewport, and a per-char escape sequence made frames heavy
        // enough to visibly flicker and to starve input under a log flood.
        fn flush(rendered: &mut String, style: Style, run: &mut String) {
            if !run.is_empty() {
                rendered.push_str(&style.paint(run.as_str()).to_string());
                run.clear();
            }
        }

        let width = self.width.max(1);
        let (window_start, window_end, indent) = if wrap_row == 0 {
            (0, width, 0)
        } else {
            let continuation = self.continuation_width();
            let start = width + (wrap_row - 1) * continuation;
            (start, start + continuation, width - continuation)
        };
        let mut rendered = " ".repeat(indent);
        let mut run = String::new();
        let mut run_style = Style::new();
        for (&(character, base), overlay_slot) in chars
            .iter()
            .zip(&overlays)
            .take(window_end)
            .skip(window_start)
        {
            let style = if let Some(overlay) = *overlay_slot {
                overlay
            } else {
                base
            };
            if style != run_style {
                flush(&mut rendered, run_style, &mut run);
                run_style = style;
            }
            run.push(character);
        }
        flush(&mut rendered, run_style, &mut run);
        rendered
    }
}

/// `wl-copy` under Wayland, then `xclip`/`xsel` under X11. False when no tool
/// is present (or none succeeded) — the caller falls back to OSC 52.
fn copy_with_system_tool(text: &str) -> bool {
    let candidates: &[(&str, &[&str], &str)] = &[
        ("wl-copy", &[], "WAYLAND_DISPLAY"),
        ("xclip", &["-selection", "clipboard", "-in"], "DISPLAY"),
        ("xsel", &["--clipboard", "--input"], "DISPLAY"),
    ];
    for (program, arguments, display_variable) in candidates {
        if std::env::var_os(display_variable).is_none() {
            continue;
        }
        let Ok(mut child) = std::process::Command::new(program)
            .args(*arguments)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take()
            && stdin.write_all(text.as_bytes()).is_err()
        {
            // Reap and move on — a failure here only means trying the next
            // tool, with OSC 52 as the final fallback.
            let _ = child.wait();
            continue;
        }
        match child.wait() {
            Ok(status) if status.success() => return true,
            _ => continue,
        }
    }
    false
}

fn copy_summary(selection: &Selection) -> String {
    let Selection { start, end, kind } = *selection;
    let lines = end.0 - start.0 + 1;
    match kind {
        VisualKind::Block => {
            let (low, high) = selection.columns();
            format!("copied {lines}x{} block to clipboard", high - low + 1)
        }
        _ if kind == VisualKind::Line || lines > 1 => {
            format!(
                "copied {lines} line{} to clipboard",
                if lines == 1 { "" } else { "s" }
            )
        }
        _ => format!("copied {} chars to clipboard", end.1 - start.1 + 1),
    }
}

/// Char positions where `pattern` starts inside `chars`.
fn matches_in(chars: &[char], pattern: &str) -> Vec<usize> {
    let pattern_chars: Vec<char> = pattern.chars().collect();
    if pattern_chars.is_empty() || chars.len() < pattern_chars.len() {
        return Vec::new();
    }
    (0..=chars.len() - pattern_chars.len())
        .filter(|&start| chars[start..start + pattern_chars.len()] == pattern_chars[..])
        .collect()
}
