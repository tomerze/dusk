use std::boxed::Box;
use std::string::{String, ToString};
use std::vec::Vec;
mod builtins;
pub mod display_engine;
mod highlighter;
pub mod stream;
pub mod ui;

use crate::entry::{EntryInfo, GetAvailableProgramsInfo, ShEntriesBuilder};
use dusk_program::anyhow::Result;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{format, println};
use tokio::sync::Notify;

use nu_ansi_term::{Color, Style};
use reedline::{EditCommand, Reedline};

use crossterm::cursor::{MoveDown, MoveTo, MoveToColumn, MoveUp, position as cursor_position};
use crossterm::style::{Color as CrosstermColor, Print, ResetColor, SetForegroundColor};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{event::DisableBracketedPaste, execute};
use reedline::Signal;
use std::io::stdout;

use crate::client::shell::Shell;
use display_engine::{DefaultDisplayEngine, DisplayEngine};
use dusk_llm::Chat;
use ui::spinner::with_spinner;
use ui::{
    CommandPrompt, PromptModeFlag, TOGGLE_CHAT_HOST_COMMAND, get_line_editor,
    render_keepalive_suffix,
};

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
    llm_chat: Option<Chat>,
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
        let mut available_programs_info = builtins::BUILTINS.to_vec();

        available_programs_info.extend(get_available_programs_info.get_available_programs_info()?);

        Ok(Prompt {
            shell,
            available_entries_info: available_programs_info,
            display_engine,
            stream_factory,
            stop_signal,
            llm_chat: None,
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
        let stripped = crate::parser::strip_comments(line);
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

        // A comment-only (or blank) line executes nothing — keep its
        // indicator dim and its history item free of execution metadata.
        let runs_command = !crate::parser::strip_comments(buffer).trim().is_empty();
        if runs_command {
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

        if runs_command {
            line_editor.update_last_command_context(&|mut history_item| {
                history_item.duration = Some(duration);
                history_item.exit_status = Some(0);
                history_item
            })?;
        }

        Ok(false)
    }

    /// Build the chat if there isn't one yet, so Ask Dusk is known to be usable
    /// *before* the prompt accepts a question. Doing this at submit time would
    /// mean discarding something the user had already typed.
    fn prepare_chat(&mut self) -> Result<()> {
        if self.llm_chat.is_some() {
            return Ok(());
        }
        let mut entries: Vec<&EntryInfo> = self.available_entries_info.iter().collect();
        entries.sort_by_key(|entry| entry.name);
        let mut programs = String::new();
        for entry in entries {
            programs += &format!("\n## {} — {}\n", entry.name, entry.short_description);
            let long_description = entry.long_description.trim();
            if !long_description.is_empty() {
                programs += long_description;
                programs.push('\n');
            }
        }
        self.llm_chat = Some(Chat::new(&programs)?);
        Ok(())
    }

    async fn chat_submit(&mut self, natural_language: &str) -> Option<String> {
        let natural_language = natural_language.trim();
        if natural_language.is_empty() {
            return None;
        }

        let Some(llm_chat) = self.llm_chat.as_ref() else {
            tracing::warn!("ask dusk was asked a question without a prepared chat");
            return None;
        };

        let token_count = Arc::new(AtomicUsize::new(0));
        let writer = token_count.clone();
        let result = with_spinner(
            llm_chat.chat(natural_language, move |count| {
                writer.store(count, Ordering::Relaxed)
            }),
            move || {
                // An endpoint that reasons before answering sends nothing at
                // all until it is done — measured at 3 to 17 seconds against
                // Gemini. A count of zero for that whole stretch reads as a
                // stall, so the wait is named instead of tallied.
                let progress = match token_count.load(Ordering::Relaxed) {
                    0 => String::from("· waiting for tokens"),
                    count => format!("· ↓ {count} tokens"),
                };
                format!(
                    "{} {}",
                    Style::new().fg(Color::Yellow).paint("Dusking…"),
                    Style::new().fg(Color::DarkGray).paint(progress),
                )
            },
        )
        .await;

        let reply = match result {
            Ok(reply) => reply,
            Err(error) => {
                tracing::warn!("{error:#}");
                return None;
            }
        };

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
        let function_names: crate::client::prompt::highlighter::FunctionNames =
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
                Err(e) => tracing::trace!(error = %e, "failed to fetch shell functions"),
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
                    // Entering is where Ask Dusk has to prove it can answer. If
                    // it can't, say so now and stay in command mode — the
                    // alternative is taking a question and then binning it.
                    match self.mode.is_chat() {
                        false => match self.prepare_chat() {
                            Ok(()) => self.mode.toggle(),
                            Err(error) => tracing::warn!("{error:#}"),
                        },
                        true => self.mode.toggle(),
                    }
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
                        .process_line(&buffer, &mut line_editor, status_plain_width)
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

pub fn serve<S: ShEntriesBuilder>(
    client: dusk_capnp::dusk_capnp::dusk::Client,
    process: dusk_capnp::dusk_capnp::process::Client,
    sh_entries_builder: S,
) {
    tokio::task::spawn_local(async move {
        if let Err(error) = serve_prompt(client, process, sh_entries_builder).await {
            tracing::error!("served sh prompt failed: {error:#}");
        }
    });
}

fn kill_abandoned(
    client: dusk_capnp::dusk_capnp::dusk::Client,
    process: dusk_capnp::dusk_capnp::process::Client,
) {
    tokio::task::spawn_local(async move {
        let killed = async {
            let pid = process
                .pid_request()
                .send()
                .promise
                .await?
                .get()?
                .get_result();
            let mut kill_request = client.kill_request();
            kill_request.get().set_pid(pid);
            kill_request.get().set_signal(15); // SIGTERM
            kill_request.send().promise.await?;
            Ok::<u64, capnp::Error>(pid)
        }
        .await;
        match killed {
            Ok(pid) => tracing::info!(pid, "killed an sh no prompt could be opened on"),
            Err(error) => tracing::error!("failed to kill an unserved sh: {error:#}"),
        }
    });
}

async fn serve_prompt<S: ShEntriesBuilder>(
    client: dusk_capnp::dusk_capnp::dusk::Client,
    process: dusk_capnp::dusk_capnp::process::Client,
    sh_entries_builder: S,
) -> Result<()> {
    let abandoned = process.clone();
    let adopted = Shell::adopt(
        client.clone(),
        sh_entries_builder.clone(),
        process,
        crate::parser::Parser::new(),
    )
    .await;
    let mut shell = match adopted.inspect_err(|_| kill_abandoned(client, abandoned))? {
        Some(shell) => shell,
        None => return Ok(()),
    };
    tracing::info!("prompt open");
    let stop_signal = Rc::new(Notify::new());

    let stream_factory = |request: StreamRequest<DefaultDisplayEngine>| match request {
        StreamRequest::Raw => {
            let (json_stream, done_receiver) =
                stream::json_stream::JsonStream::new_with_receiver(true);
            (capnp_rpc::new_client(json_stream), done_receiver)
        }
        StreamRequest::Display { display_engine } => {
            let (display_stream, done_receiver) =
                stream::display_stream::DisplayStream::new_with_receiver(display_engine.clone());
            (capnp_rpc::new_client(display_stream), done_receiver)
        }
    };

    let result = async {
        let prompt = Prompt::new(
            &mut shell,
            sh_entries_builder,
            DefaultDisplayEngine::default(),
            stream_factory,
            stop_signal,
        )
        .await?;
        prompt.run().await
    }
    .await;
    if let Err(error) = shell.kill().await {
        tracing::error!("failed to kill the served sh: {error:#}");
    }
    tracing::info!("prompt closed");
    result
}
