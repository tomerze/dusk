use std::borrow::Cow;

use dusk_program::anyhow::{Result, anyhow};

use crossterm::{
    cursor::SetCursorStyle,
    event::{KeyCode, KeyModifiers},
};
use nu_ansi_term::{Color, Style};
use reedline::{
    ColumnarMenu, CursorConfig, DefaultCompleter, DefaultHinter, DefaultValidator, EditCommand,
    Keybindings, ListMenu, MenuBuilder, PromptEditMode, PromptHistorySearch,
    PromptHistorySearchStatus, Reedline, ReedlineEvent, ReedlineMenu, Vi,
    default_vi_insert_keybindings, default_vi_normal_keybindings,
};

use crate::highlighter::CustomHighlighter;
use dusk_shell::shell::RttHandle;

static DEFAULT_MULTILINE_INDICATOR: &str = "::: ";

#[derive(Clone)]
pub(crate) struct ReedlinePrompt {
    status_template: String,
    indicator_line: String,
    rtt_handle: RttHandle,
}

impl ReedlinePrompt {
    pub(crate) fn new(
        status_template: String,
        indicator_line: String,
        rtt_handle: RttHandle,
    ) -> Self {
        ReedlinePrompt {
            status_template,
            indicator_line,
            rtt_handle,
        }
    }
}

pub(crate) fn render_keepalive_suffix(rtt_handle: &RttHandle) -> String {
    match *rtt_handle.lock().unwrap() {
        Some(rtt) => Style::new()
            .fg(Color::DarkGray)
            .paint(format!(" ⇄ {}ms", rtt.as_millis()))
            .to_string(),
        None => Style::new()
            .fg(Color::Red)
            .bold()
            .paint(" disconnected")
            .to_string(),
    }
}

impl reedline::Prompt for ReedlinePrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Owned(format!(
            "{}{}\n{}",
            self.status_template,
            render_keepalive_suffix(&self.rtt_handle),
            self.indicator_line
        ))
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, _edit_mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Owned(format!(
            " {} ",
            Style::new().fg(Color::LightGreen).paint("❯")
        ))
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

pub(crate) fn get_line_editor(
    commands: Vec<String>,
    functions: crate::highlighter::FunctionNames,
) -> Result<Reedline> {
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
        functions,
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
        KeyCode::Char('r'),
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
