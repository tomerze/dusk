use chrono::Duration;
use dusk_program::anyhow::{anyhow, Result};
use dusk_program_sh::entry::{GetAvailableProgramsInfo, ProgramInfo};
use std::{borrow::Cow, cell::Cell};

use crossterm::{
    cursor::SetCursorStyle,
    event::{KeyCode, KeyModifiers},
};
use nu_ansi_term::{Color, Style};
use nu_color_config::TextStyle;
use nu_table::{NuRecordsValue, NuTable, TableTheme};
use pretty_duration::pretty_duration;
use reedline::{
    default_vi_insert_keybindings, default_vi_normal_keybindings, ColumnarMenu, DefaultCompleter,
    DefaultHinter, DefaultValidator, EditCommand, Keybindings, ListMenu, MenuBuilder,
    PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus, Reedline, ReedlineEvent,
    ReedlineMenu, Vi,
};

use reedline::CursorConfig;

use crossterm::cursor::{MoveDown, MoveToColumn, MoveUp};
use crossterm::style::{Color as CrosstermColor, Print, ResetColor, SetForegroundColor};
use crossterm::{event::DisableBracketedPaste, execute};
use reedline::Signal;
use std::io::stdout;

use crate::display_engine::DisplayEngine;
use crate::highlighter::CustomHighlighter;
use dusk_shell::shell::Shell;

#[derive(Clone)]
struct ReedlinePrompt<'s> {
    left_prompt: Cow<'s, str>,
    right_prompt: Cell<Duration>,
}

impl<'s> ReedlinePrompt<'s> {
    pub fn new(prompt_str: &'s str) -> Self {
        ReedlinePrompt {
            left_prompt: Cow::Owned(prompt_str.to_string()),
            right_prompt: Cell::new(Duration::zero()),
        }
    }
}

static DEFAULT_MULTILINE_INDICATOR: &str = "::: ";

impl<'s> reedline::Prompt for ReedlinePrompt<'s> {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Owned(self.left_prompt.to_string())
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        match self.right_prompt.get().to_std() {
            Ok(duration) => {
                let duration_str = pretty_duration(&duration, None);
                Cow::Owned(format!("[{duration_str}]"))
            }
            Err(_err) => Cow::Owned(String::from("[error]")),
        }
    }

    fn render_prompt_indicator(&self, _edit_mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Owned(" ❯ ".to_string())
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed(DEFAULT_MULTILINE_INDICATOR)
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        let prefix = match history_search.status {
            PromptHistorySearchStatus::Passing => "",
            PromptHistorySearchStatus::Failing => "failing ",
        };

        Cow::Owned(format!(
            " ({}reverse-search: {}) ",
            prefix, history_search.term
        ))
    }

    fn right_prompt_on_last_line(&self) -> bool {
        false
    }
}

fn get_line_editor(commands: Vec<String>) -> Result<Reedline> {
    let history = Box::new(
        reedline::SqliteBackedHistory::with_file("history.sqlite3".into(), None, None)
            .map_err(|_err| anyhow!("failed to open history db"))?,
    );

    let completer = Box::new(DefaultCompleter::new_with_wordlen(commands.clone(), 2));

    let cursor_config = CursorConfig {
        vi_insert: Some(SetCursorStyle::BlinkingBar),
        vi_normal: Some(SetCursorStyle::SteadyBlock),
        emacs: None,
    };

    let highlighter = CustomHighlighter {
        external_commands: commands,
    };
    let mut line_editor = Reedline::create()
        .with_history_session_id(None)
        .with_history(history)
        .with_history_exclusion_prefix(Some(" ".to_string()))
        .with_completer(completer)
        .with_quick_completions(true)
        .with_partial_completions(true)
        .with_cursor_config(cursor_config)
        .with_highlighter(Box::new(highlighter))
        .with_hinter(Box::new(
            DefaultHinter::default().with_style(Style::new().fg(Color::DarkGray)),
        ))
        .with_validator(Box::new(DefaultValidator))
        .with_ansi_colors(true)
        .with_menu(ReedlineMenu::EngineCompleter(Box::new(
            ColumnarMenu::default().with_name("completion_menu"),
        )))
        .with_menu(ReedlineMenu::HistoryMenu(Box::new(
            ListMenu::default().with_name("history_menu"),
        )))
        .use_bracketed_paste(true);

    let mut normal_keybindings = default_vi_normal_keybindings();
    let mut insert_keybindings = default_vi_insert_keybindings();

    add_menu_keybindings(&mut normal_keybindings);
    add_menu_keybindings(&mut insert_keybindings);

    add_newline_keybinding(&mut insert_keybindings);

    let edit_mode = Vi::new(insert_keybindings, normal_keybindings);

    line_editor = line_editor.with_edit_mode(Box::new(edit_mode));

    line_editor = line_editor.with_buffer_editor(
        std::process::Command::new("vi"),
        std::env::temp_dir().join("tmp"),
    );

    Ok(line_editor)
}

fn add_menu_keybindings(keybindings: &mut Keybindings) {
    keybindings.add_binding(
        KeyModifiers::CONTROL,
        KeyCode::Char('x'),
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu("history_menu".to_string()),
            ReedlineEvent::MenuPageNext,
        ]),
    );

    keybindings.add_binding(
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        KeyCode::Char('x'),
        ReedlineEvent::MenuPagePrevious,
    );

    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu("completion_menu".to_string()),
            ReedlineEvent::Edit(vec![EditCommand::Complete]),
        ]),
    );

    keybindings.add_binding(
        KeyModifiers::SHIFT,
        KeyCode::BackTab,
        ReedlineEvent::MenuPrevious,
    );
}

fn add_newline_keybinding(keybindings: &mut Keybindings) {
    // This doesn't work for macOS
    keybindings.add_binding(
        KeyModifiers::ALT,
        KeyCode::Enter,
        ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
    );
}

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
    available_programs_info: Vec<ProgramInfo>,
    display_engine: D,
    stream_factory: F,
}

impl<'a, D, F> Prompt<'a, D, F>
where
    D: DisplayEngine + Clone + 'static,
    F: for<'d> Fn(StreamRequest<'d, D>) -> (dusk_capnp::dusk_capnp::stream::Client, DoneReceiver),
{
    const BUILTIN_COMMANDS: [ProgramInfo; 3] = [
        ProgramInfo {
            name: "clear",
            version: "builtin",
            program_id: None,
            short_description: "clear the screen",
            long_description: r#"Example,
`clear`"#,
        },
        ProgramInfo {
            name: "exit",
            version: "builtin",
            program_id: None,
            short_description: "exit the shell",
            long_description: r#"Example,
`exit`"#,
        },
        ProgramInfo {
            name: "help",
            version: "builtin",
            program_id: None,
            short_description: "help, try `help help`",
            long_description: r#"
The `help` command displays information about available commands.
* Use `help` to list all available commands.
* Use `help <command>` to get more information about a specific command.
"#,
        },
    ];

    pub async fn new(
        shell: &'a mut Shell,
        get_available_programs_info: impl GetAvailableProgramsInfo,
        display_engine: D,
        stream_factory: F,
    ) -> Result<Self> {
        let mut available_programs_info = Self::BUILTIN_COMMANDS.to_vec();

        available_programs_info.extend(get_available_programs_info.get_available_programs_info()?);

        Ok(Prompt {
            shell,
            available_programs_info,
            display_engine,
            stream_factory,
        })
    }

    fn get_available_commands_table(&self) -> Result<String> {
        let mut table = NuTable::new(self.available_programs_info.len() + 1, 3);
        let headers = vec![
            NuRecordsValue::new("Command".into()),
            NuRecordsValue::new("Description".into()),
            NuRecordsValue::new("Local Version".into()),
        ];
        table.set_row(0, headers);
        for (i, command) in self.available_programs_info.iter().enumerate() {
            let row = vec![
                NuRecordsValue::new(
                    self.display_engine
                        .render_markdown_inline(format!("**{}**", command.name).as_str()),
                ),
                NuRecordsValue::new(
                    self.display_engine
                        .render_markdown_inline(command.short_description),
                ),
                NuRecordsValue::new(
                    self.display_engine
                        .render_markdown_inline(format!("`{}`", command.version).as_str()),
                ),
            ];
            table.set_row(i + 1, row);
        }

        table.set_data_style(TextStyle::basic_left().style(Style::new().fg(Color::DarkGray)));
        table.set_header_style(
            TextStyle::basic_center().style(Style::new().fg(Color::Yellow).bold()),
        );
        table.set_theme(TableTheme::rounded());
        table.set_structure(false, true, false);
        let width = crossterm::terminal::size()?.0 as usize;
        let table_str = table.draw(width).unwrap_or("[cannot fit]".to_string());
        Ok(table_str)
    }

    fn get_program_info_markdown(&self, program_name: &str) -> Result<Option<String>> {
        let program_info = self
            .available_programs_info
            .iter()
            .find(|p| p.name == program_name);

        let markdown = r#"# {name}
## Info:
Local version: `{version}`
Program ID: `{program_id}`
## Description:
**{short_description}**{long_description}
```"#;

        let program_info = if let Some(program_info) = program_info {
            program_info
        } else {
            return Ok(None);
        };
        let program_id = program_info
            .program_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "N/A".to_string());
        let formatted_markdown = markdown
            .replace("{name}", program_info.name)
            .replace("{version}", program_info.version)
            .replace("{program_id}", &program_id)
            .replace("{short_description}", program_info.short_description)
            .replace(
                "{long_description}",
                format!("\n{}", program_info.long_description).as_str(),
            );

        Ok(Some(
            self.display_engine
                .render_markdown(formatted_markdown.as_str()),
        ))
    }

    fn help(&self, line: &str) -> Result<()> {
        let command = line.split_whitespace().nth(1);
        let draw = if let Some(command) = command {
            if let Some(markdown) = self.get_program_info_markdown(command)? {
                markdown
            } else {
                format!("No help found for command: {}", command)
            }
        } else {
            self.get_available_commands_table()?
        };

        println!("{}", draw);

        Ok(())
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
    async fn process_line(&mut self, mut line: &str, line_editor: &mut Reedline) -> Result<bool> {
        let first_word = match line.split_whitespace().next() {
            Some(word) => word,
            None => return Ok(false),
        };
        match first_word {
            "exit" => return Ok(true),
            "clear" => {
                line_editor.clear_screen()?;
            }
            "help" => {
                self.help(line)?;
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
                if let Err(e) = self.shell.sh(line, stream, done_receiver).await {
                    tracing::error!("{:?}: {:?}", first_word, e);
                }
            }
        };

        Ok(false)
    }

    pub async fn run(mut self) -> Result<()> {
        self.run_inner().await?;
        Ok(())
    }

    async fn run_inner(&mut self) -> Result<()> {
        let mut line_editor = get_line_editor(
            self.available_programs_info
                .iter()
                .map(|p| p.name.to_string())
                .collect(),
        )?;

        let prompt_string = Style::new()
            .fg(Color::Yellow)
            .bold()
            .paint(format!("○ {}", self.shell.hostname))
            .to_string();
        let prompt = ReedlinePrompt::new(&prompt_string);

        loop {
            let sig = line_editor.read_line(&prompt)?;
            match sig {
                Signal::Success(buffer) => {
                    if !buffer.is_empty() {
                        line_editor.update_last_command_context(
                            &|mut history_item: reedline::HistoryItem| {
                                history_item.start_timestamp = Some(chrono::Utc::now());
                                history_item
                            },
                        )?;
                        // Immediately after Enter is pressed, go to start of line and replace circle
                        execute!(
                            stdout(),
                            MoveUp(1),
                            MoveToColumn(0),
                            SetForegroundColor(CrosstermColor::Yellow),
                            Print("●"),
                            ResetColor,
                            MoveDown(1),
                            MoveToColumn(0)
                        )?;
                    }
                    let start_timestamp = std::time::Instant::now();

                    let should_exit = self.process_line(&buffer, &mut line_editor).await?;
                    if should_exit {
                        return Ok(());
                    }

                    let duration = start_timestamp.elapsed();
                    prompt.right_prompt.set(Duration::from_std(duration)?);

                    if !buffer.is_empty() {
                        line_editor.update_last_command_context(&|mut history_item| {
                            history_item.duration = Some(duration);
                            history_item.exit_status = Some(0);
                            history_item
                        })?;
                    }
                }
                Signal::CtrlD | Signal::CtrlC => {
                    break;
                }
            }
        }

        execute!(stdout(), DisableBracketedPaste)?;

        Ok(())
    }
}
