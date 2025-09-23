use anyhow::{anyhow, Result};
use chrono::Duration;
use dusk_program_sh::program_args_builder::ProgramInfo;
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
    DefaultHinter, DefaultValidator, EditCommand, ExampleHighlighter, Keybindings, ListMenu,
    MenuBuilder, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus, Reedline,
    ReedlineEvent, ReedlineMenu, Vi,
};

use reedline::CursorConfig;

use crossterm::{event::DisableBracketedPaste, execute};
use reedline::Signal;
use std::io::stdout;
use tracing::info;

use crate::shell::Shell;

#[derive(Clone)]
struct ReedlinePrompt<'s> {
    left_prompt: Cow<'s, str>,
    right_prompt: Cell<Duration>,
}

impl<'s> ReedlinePrompt<'s> {
    pub fn new(prompt_name: &'s str) -> Self {
        ReedlinePrompt {
            left_prompt: Cow::Owned(format!("[{}]", prompt_name)),
            right_prompt: Cell::new(Duration::zero()),
        }
    }
}

static DEFAULT_MULTILINE_INDICATOR: &str = "::: ";

impl<'s> reedline::Prompt for ReedlinePrompt<'s> {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        {
            Cow::Owned(self.left_prompt.to_string())
        }
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
        Cow::Owned(" # ".to_string())
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
    let mut line_editor = Reedline::create()
        .with_history_session_id(None)
        .with_history(history)
        .with_history_exclusion_prefix(Some(" ".to_string()))
        .with_completer(completer)
        .with_quick_completions(true)
        .with_partial_completions(true)
        .with_cursor_config(cursor_config)
        .with_highlighter(Box::new(ExampleHighlighter::new(commands)))
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

pub struct Prompt {
    shell: Shell,
    available_programs_info: Vec<ProgramInfo>,
}

impl Prompt {
    const BUILTIN_COMMANDS: [ProgramInfo; 3] = [
        ProgramInfo {
            name: "clear",
            version: "builtin",
            program_id: None,
            short_description: "clear the screen",
            long_description: "",
        },
        ProgramInfo {
            name: "exit",
            version: "builtin",
            program_id: None,
            short_description: "exit the shell",
            long_description: "",
        },
        ProgramInfo {
            name: "help",
            version: "builtin",
            program_id: None,
            short_description: "this table",
            long_description: "",
        },
    ];

    pub async fn new(shell: Shell) -> Result<Self> {
        let mut available_programs_info = Self::BUILTIN_COMMANDS.to_vec();
        let shell_available_programs_info = shell.get_available_programs_info();
        available_programs_info.extend(shell_available_programs_info);
        Ok(Prompt {
            shell,
            available_programs_info,
        })
    }

    fn get_available_commands_table(&self) -> NuTable {
        let mut table = NuTable::new(self.available_programs_info.len() + 1, 3);
        let headers = vec![
            NuRecordsValue::new("Command".into()),
            NuRecordsValue::new("Description".into()),
            NuRecordsValue::new("Local Version".into()),
        ];
        table.set_row(0, headers);
        for (i, command) in self.available_programs_info.iter().enumerate() {
            let row = vec![
                NuRecordsValue::new(command.name.to_string()),
                NuRecordsValue::new(command.short_description.to_string()),
                NuRecordsValue::new(command.version.to_string()),
            ];
            table.set_row(i + 1, row);
        }

        table.set_data_style(TextStyle::basic_left().style(Style::new().fg(Color::Green)));
        table.set_header_style(
            TextStyle::basic_center().style(Style::new().fg(Color::Yellow).bold()),
        );
        table.set_theme(TableTheme::rounded());
        table.set_structure(false, true, false);
        table
    }

    fn get_program_info_table(&self, program_name: &str) -> NuTable {
        let program_info = self
            .available_programs_info
            .iter()
            .find(|p| p.name == program_name);
        let mut table = NuTable::new(4, 1);

        if let Some(program_info) = program_info {
            table.set_row(0, vec![NuRecordsValue::new(program_info.name.to_string())]);

            table.set_row(
                1,
                vec![NuRecordsValue::new(format!(
                    "Version: {}",
                    program_info.version
                ))],
            );

            table.set_row(
                2,
                vec![NuRecordsValue::new(format!(
                    "Program ID: {}",
                    program_info.program_id.unwrap_or(0)
                ))],
            );

            table.set_row(
                3,
                vec![NuRecordsValue::new(format!(
                    r#"Description: {} {}"#,
                    program_info.short_description, program_info.long_description
                ))],
            );
        } else {
            table.set_row(
                0,
                vec![
                    NuRecordsValue::new("Error".into()),
                    NuRecordsValue::new(format!("Program '{}' not found.", program_name)),
                ],
            );
        }
        table.set_data_style(TextStyle::basic_left().style(Style::new().fg(Color::Green)));
        table.set_header_style(
            TextStyle::basic_center().style(Style::new().fg(Color::Yellow).bold()),
        );
        table.set_border_header(false);
        table.set_theme(TableTheme::rounded());
        table.set_structure(false, true, false);
        table
    }

    /// Handles a executing a shell command but also knows how to deal with the prompt itself.
    /// That is mainly being able to clear the prompt.
    ///
    /// Return true when prompt should exit.
    async fn process_line(&mut self, line: &str, line_editor: &mut Reedline) -> Result<bool> {
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
                let command = line.split_whitespace().nth(1);
                let table = if let Some(command) = command {
                    self.get_program_info_table(command)
                } else {
                    self.get_available_commands_table()
                };
                let width = crossterm::terminal::size()?.0 as usize;
                let table_str = table.draw(width).unwrap_or("[cannot fit]".to_string());
                println!("{}", table_str);
            }
            command => {
                if let Err(e) = self.shell.process_command(command).await {
                    tracing::error!("{:?}: {}", command, e);
                }
            }
        };

        Ok(false)
    }

    pub async fn run(mut self) -> Result<()> {
        self.run_inner().await?;
        self.shell.kill().await?;
        Ok(())
    }

    async fn run_inner(&mut self) -> Result<()> {
        let mut line_editor = get_line_editor(
            self.available_programs_info
                .iter()
                .map(|p| p.name.to_string())
                .collect(),
        )?;

        let hostname_clone = self.shell.hostname.clone();
        let prompt = ReedlinePrompt::new(&hostname_clone);

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
                    info!("aborted");
                    break;
                }
            }
        }

        execute!(stdout(), DisableBracketedPaste)?;

        Ok(())
    }
}
