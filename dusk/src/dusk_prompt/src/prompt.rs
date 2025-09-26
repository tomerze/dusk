use anyhow::{anyhow, Result};
use chrono::Duration;
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
    DefaultHinter, DefaultValidator, EditCommand, ExampleHighlighter, Keybindings, ListMenu,
    MenuBuilder, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus, Reedline,
    ReedlineEvent, ReedlineMenu, Vi,
};

use reedline::CursorConfig;

use crossterm::{event::DisableBracketedPaste, execute};
use reedline::Signal;
use std::io::stdout;
use tracing::info;

use crate::{display_stream, shell::Shell};

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
    markdown_skin: termimad::MadSkin,
}

impl Prompt {
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
        shell: Shell,
        get_available_programs_info: impl GetAvailableProgramsInfo,
    ) -> Result<Self> {
        let mut available_programs_info = Self::BUILTIN_COMMANDS.to_vec();

        available_programs_info.extend(get_available_programs_info.get_available_programs_info()?);

        let mut markdown_skin = termimad::MadSkin::default();
        use termimad::crossterm::style::Color;
        markdown_skin.paragraph.set_fg(termimad::rgb(30, 30, 40));
        markdown_skin.bold.set_fg(Color::Grey);
        markdown_skin.headers[1].set_fg(Color::Yellow);
        markdown_skin.bullet.set_char('○');
        markdown_skin.bullet.set_fg(Color::DarkYellow);
        markdown_skin.inline_code.set_fg(Color::Cyan);
        markdown_skin.code_block.set_fg(Color::Cyan);

        Ok(Prompt {
            shell,
            available_programs_info,
            markdown_skin,
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
                    self.markdown_skin
                        .inline(format!("**{}**", command.name).as_str())
                        .to_string(),
                ),
                NuRecordsValue::new(
                    self.markdown_skin
                        .inline(command.short_description)
                        .to_string(),
                ),
                NuRecordsValue::new(
                    self.markdown_skin
                        .inline(format!("`{}`", command.version).as_str())
                        .to_string(),
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

    fn get_program_info_markdown(&self, program_name: &str) -> Result<String> {
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
        let program_info = program_info.ok_or(anyhow::anyhow!("Program not found"))?;
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

        Ok(self
            .markdown_skin
            .term_text(formatted_markdown.as_str())
            .to_string())
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
                let draw = if let Some(command) = command {
                    self.get_program_info_markdown(command)?
                } else {
                    self.get_available_commands_table()?
                };

                println!("{}", draw);
            }
            command => {
                let (display_stream, done_receiver) =
                    display_stream::DisplayStream::new_with_receiver(&self.markdown_skin);
                let display_stream: dusk_capnp::dusk_capnp::stream::Client =
                    capnp_rpc::new_client(display_stream);
                if let Err(e) = self
                    .shell
                    .process_command(command, display_stream, done_receiver)
                    .await
                {
                    tracing::error!("{:?}: {:?}", command, e);
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
