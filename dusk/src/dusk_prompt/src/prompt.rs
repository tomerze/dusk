use dusk_program::anyhow::Result;
use dusk_program_sh::entry::{EntryInfo, GetAvailableProgramsInfo};
use std::rc::Rc;
use tokio::sync::Notify;

use nu_ansi_term::{Color, Style};
use reedline::{EditCommand, Reedline};

use crossterm::cursor::{MoveDown, MoveTo, MoveToColumn, MoveUp, position as cursor_position};
use crossterm::style::{Color as CrosstermColor, Print, ResetColor, SetForegroundColor};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{event::DisableBracketedPaste, execute};
use reedline::Signal;
use std::io::stdout;

use crate::builtins;
use crate::display_engine::DisplayEngine;
use crate::llm::{Llm, build_system_prompt};
use crate::reedline::{
    CommandPrompt, PromptModeFlag, TOGGLE_CHAT_HOST_COMMAND, get_line_editor,
    render_keepalive_suffix,
};
use dusk_shell::shell::Shell;

type DoneReceiver = tokio::sync::oneshot::Receiver<()>;

pub enum StreamRequest<'a, D>
where
    D: DisplayEngine + 'a,
{
    Raw,
    Display { display_engine: &'a D },
}

pub struct Prompt<'a, D, F>
where
    D: DisplayEngine + Clone + 'static,
    F: for<'d> Fn(StreamRequest<'d, D>) -> (dusk_capnp::dusk_capnp::stream::Client, DoneReceiver),
{
    shell: &'a mut Shell,
    available_entries_info: Vec<EntryInfo>,
    display_engine: D,
    stream_factory: F,
    /// Fired by the caller (e.g. on ctrl+c) to ask the running command to stop.
    stop_signal: Rc<Notify>,
    llm: Llm,
    mode: PromptModeFlag,
}


impl<'a, D, F> Prompt<'a, D, F>
where
    D: DisplayEngine + Clone + 'static,
    F: for<'d> Fn(StreamRequest<'d, D>) -> (dusk_capnp::dusk_capnp::stream::Client, DoneReceiver),
{
    pub async fn new(
        shell: &'a mut Shell,
        get_available_programs_info: impl GetAvailableProgramsInfo,
        display_engine: D,
        stream_factory: F,
        stop_signal: Rc<Notify>,
    ) -> Result<Self> {
        let mut available_programs_info = builtins::BUILTIN_COMMANDS.to_vec();

        available_programs_info.extend(get_available_programs_info.get_available_programs_info()?);

        let system_prompt = build_system_prompt(&available_programs_info);
        let llm = Llm::new(system_prompt);

        Ok(Prompt {
            shell,
            available_entries_info: available_programs_info,
            display_engine,
            stream_factory,
            stop_signal,
            llm,
            mode: PromptModeFlag::default(),
        })
    }

    fn get_stream(&self, is_raw: bool) -> (dusk_capnp::dusk_capnp::stream::Client, DoneReceiver) {
        if is_raw {
            (self.stream_factory)(StreamRequest::Raw)
        } else {
            (self.stream_factory)(StreamRequest::Display {
                display_engine: &self.display_engine,
            })
        }
    }

    /// Handles a executing a shell command but also knows how to deal with the prompt itself.
    /// That is mainly being able to clear the prompt.
    ///
    /// Return true when prompt should exit.
    async fn execute_command(&mut self, line: &str, line_editor: &mut Reedline) -> Result<bool> {
        let stripped = dusk_program_sh::parser::strip_comments(line);
        let mut line = stripped.as_str();
        let first_word = match line.split_whitespace().next() {
            Some(word) => word,
            None => return Ok(false),
        };
        match first_word {
            "exit" => return Ok(true),
            "clear" => {
                builtins::clear(line_editor)?;
            }
            "help" => {
                builtins::help(line, &self.available_entries_info, &self.display_engine)?;
            }
            "functions" => {
                builtins::print_functions(self.shell, &self.display_engine).await?;
            }
            _sh_entry_name => {
                // get last word of line
                let last_word = line.split_whitespace().last().unwrap_or("");
                let mut is_raw = false; // displays in json
                if last_word == "?" {
                    // remove last word from line
                    line = line[..line.rfind('?').unwrap_or(0)].trim_end();
                    is_raw = true;
                }

                let (stream, done_receiver) = self.get_stream(is_raw);
                if let Err(e) = self
                    .shell
                    .sh(line, stream, done_receiver, self.stop_signal.clone())
                    .await
                {
                    tracing::error!("{:?} error:\n{:?}", first_word, e);
                }
            }
        };

        Ok(false)
    }

    /// Number of visual lines `buffer` occupies in the prompt — used by
    /// the relative post-`read_line` edits to climb back over the
    /// buffer to the indicator / status rows. Multi-line buffers (via
    /// Alt+Enter) get counted by literal newlines; we don't try to
    /// guess at terminal-width wrapping.
    fn buffer_visual_lines(buffer: &str) -> u16 {
        (buffer.matches('\n').count() + 1) as u16
    }

    /// Strip the live-RTT suffix from the now-historical status line so
    /// scrollback doesn't keep a stale latency readout. Cursor sits at
    /// the line below the buffer after `read_line` returns, so we walk
    /// up `buffer_lines + 1` rows to reach the status row.
    fn strip_rtt_suffix(buffer: &str, status_plain_width: u16) -> Result<()> {
        let up = Self::buffer_visual_lines(buffer) + 1;
        execute!(
            stdout(),
            MoveUp(up),
            MoveToColumn(status_plain_width),
            Clear(ClearType::UntilNewLine),
            MoveDown(up),
            MoveToColumn(0),
        )?;
        Ok(())
    }

    /// Swap the dim `○` indicator at the start of the indicator row for
    /// a solid coloured `●`, marking the prompt as historical. The
    /// indicator row sits `buffer_lines` above current cursor.
    fn mark_indicator_historical(buffer: &str) -> Result<()> {
        let up = Self::buffer_visual_lines(buffer);
        execute!(
            stdout(),
            MoveUp(up),
            MoveToColumn(0),
            SetForegroundColor(CrosstermColor::Cyan),
            Print("●"),
            ResetColor,
            MoveDown(up),
            MoveToColumn(0),
        )?;
        Ok(())
    }

    /// Handle a single accepted line from reedline. Returns `true` when the prompt should exit.
    async fn process_line(
        &mut self,
        buffer: &str,
        line_editor: &mut Reedline,
        status_plain_width: u16,
    ) -> Result<bool> {
        Self::strip_rtt_suffix(buffer, status_plain_width)?;

        if !buffer.is_empty() {
            line_editor.update_last_command_context(
                &|mut history_item: reedline::HistoryItem| {
                    history_item.start_timestamp = Some(chrono::Utc::now());
                    history_item
                },
            )?;
            Self::mark_indicator_historical(buffer)?;
        }
        let start_timestamp = std::time::Instant::now();

        let should_exit = self.execute_command(buffer, line_editor).await?;
        if should_exit {
            return Ok(true);
        }

        let duration = start_timestamp.elapsed();

        if !buffer.is_empty() {
            line_editor.update_last_command_context(&|mut history_item| {
                history_item.duration = Some(duration);
                history_item.exit_status = Some(0);
                history_item
            })?;
        }

        Ok(false)
    }

    async fn chat_submit(&mut self, natural_language: &str) -> Option<String> {
        let reply = self.llm.ask(natural_language).await?;
        println!(
            "{} {}",
            Style::new().fg(Color::White).paint("●"),
            Style::new().fg(Color::White).paint(&reply.explanation),
        );
        let command = reply.command.trim().to_string();
        if command.is_empty() {
            None
        } else {
            Some(command)
        }
    }

    pub async fn run(mut self) -> Result<()> {
        let function_names: crate::highlighter::FunctionNames =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut line_editor = get_line_editor(
            self.available_entries_info
                .iter()
                .map(|p| p.name.to_string())
                .collect(),
            function_names.clone(),
        )?;

        let status_template: String = format!(
            "{}{}{}{}",
            Style::new().fg(Color::Yellow).bold().paint("dusk "),
            Style::new().fg(Color::Cyan).paint(dusk_capnp::VERSION),
            Style::new().fg(Color::Yellow).bold().paint(" @"),
            Style::new().fg(Color::Cyan).paint(&self.shell.hostname),
        );
        let status_plain_width: u16 = {
            use unicode_width::UnicodeWidthStr;
            let plain = format!("dusk {} @{}", dusk_capnp::VERSION, self.shell.hostname);
            UnicodeWidthStr::width(plain.as_str()) as u16
        };

        let indicator_line: String = Style::new().fg(Color::DarkGray).paint("○").to_string();
        let mut prompt = CommandPrompt::new(
            status_template.clone(),
            indicator_line,
            self.shell.rtt_handle.clone(),
            self.mode.clone(),
        );

        let rtt_handle_for_idle = self.shell.rtt_handle.clone();
        line_editor = line_editor
            .with_poll_interval(std::time::Duration::from_millis(50))
            .with_idle_callback(Box::new(move || {
                use std::io::Write as _;
                let payload = format!(
                    "\x1b[s\x1b[1F\x1b[2K{}{}\x1b[u",
                    status_template,
                    render_keepalive_suffix(&rtt_handle_for_idle)
                );
                let mut handle = stdout().lock();
                let _ = handle.write_all(payload.as_bytes());
                let _ = handle.flush();
            }));

        loop {
            // Refresh the live function list before each prompt so the highlighter
            // marks function names that have been defined / undefined since.
            match self.shell.functions().await {
                Ok(symbols) => *function_names.lock().unwrap() = symbols,
                Err(e) => tracing::warn!(error = %e, "failed to fetch shell functions"),
            }

            let (_, prompt_start_row) = cursor_position()?;
            // Run reedline on a worker thread so the LocalSet keeps polling
            // the background keepalive task while reedline blocks in
            // `event::poll`.
            let read_line_result;
            (line_editor, prompt, read_line_result) = tokio::task::spawn_blocking(move || {
                let result = line_editor.read_line(&prompt);
                (line_editor, prompt, result)
            })
            .await
            .inspect_err(|err| tracing::error!("reedline task failed: {err}"))?;
            let sig = read_line_result?;
            match sig {
                Signal::Success(buffer) if buffer == TOGGLE_CHAT_HOST_COMMAND => {
                    self.mode.toggle();
                    execute!(
                        stdout(),
                        MoveTo(0, prompt_start_row),
                        Clear(ClearType::FromCursorDown),
                    )?;
                }
                Signal::Success(buffer) if self.mode.is_chat() => {
                    Self::strip_rtt_suffix(&buffer, status_plain_width)?;
                    if !buffer.is_empty() {
                        Self::mark_indicator_historical(&buffer)?;
                    }
                    let command = self.chat_submit(&buffer).await;
                    self.mode.set_command();
                    line_editor.run_edit_commands(&[EditCommand::Clear]);
                    if let Some(command) = command {
                        line_editor.run_edit_commands(&[EditCommand::InsertString(command)]);
                    }
                }
                Signal::Success(buffer) => {
                    if self
                        .process_line(
                            &buffer,
                            &mut line_editor,
                            status_plain_width,
                        )
                        .await?
                    {
                        return Ok(());
                    }
                }
                Signal::CtrlD | Signal::CtrlC => {
                    break;
                }
                signal => {
                    tracing::warn!("unhandled signal from reedline: {:?}", signal);
                }
            }
        }

        execute!(stdout(), DisableBracketedPaste)?;

        Ok(())
    }
}
