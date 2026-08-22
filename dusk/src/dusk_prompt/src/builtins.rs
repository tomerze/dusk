use dusk_base::dusk_program_sh::entry::EntryInfo;
use dusk_program::anyhow::Result;
use nu_ansi_term::{Color, Style};
use nu_color_config::TextStyle;
use nu_table::{NuRecordsValue, NuTable, TableTheme};
use reedline::{Highlighter, Reedline};

use crate::display_engine::DisplayEngine;
use crate::highlighter::CustomHighlighter;
use dusk_shell::shell::Shell;

pub(crate) const BUILTINS: [EntryInfo; 4] = [
    EntryInfo {
        name: "clear",
        version: "builtin",
        program_id: None,
        short_description: "clear the screen",
        long_description: r#"Example,
`clear`"#,
    },
    EntryInfo {
        name: "exit",
        version: "builtin",
        program_id: None,
        short_description: "exit the shell",
        long_description: r#"Example,
`exit`"#,
    },
    EntryInfo {
        name: "help",
        version: "builtin",
        program_id: None,
        short_description: "help, try `help help`",
        long_description: r#"
The `help` builtin displays information about the programs this client can run,
and about the shell's own builtins.
* Use `help` to list every program and builtin.
* Use `help <program>` to get more information about a specific one.
"#,
    },
    EntryInfo {
        name: "functions",
        version: "builtin",
        program_id: None,
        short_description: "list all defined shell functions",
        long_description: r#"
The `functions` builtin lists all functions defined in any shell
"#,
    },
];

pub(crate) fn clear(line_editor: &mut Reedline) -> Result<()> {
    line_editor.clear_screen()?;
    Ok(())
}

pub(crate) fn help<D: DisplayEngine>(
    line: &str,
    available_programs_info: &[EntryInfo],
    display_engine: &D,
) -> Result<()> {
    let program = line.split_whitespace().nth(1);
    let draw = if let Some(program) = program {
        if let Some(markdown) =
            get_program_info_markdown(program, available_programs_info, display_engine)?
        {
            markdown
        } else {
            format!("No help found for program: {}", program)
        }
    } else {
        get_available_programs_table(available_programs_info, display_engine)?
    };

    println!("{}", draw);

    Ok(())
}

pub(crate) async fn print_functions<D: DisplayEngine>(
    shell: &mut Shell,
    display_engine: &D,
) -> Result<()> {
    let functions = shell.functions().await?;
    if functions.is_empty() {
        let example = "# defines foo function which just calls itself\nfoo() {\n    foo\n}";
        let highlighter = CustomHighlighter {
            external_commands: Vec::new(),
            functions: std::sync::Arc::new(std::sync::Mutex::new(vec!["foo".to_string()])),
        };
        let highlighted: String = example
            .lines()
            .map(|line| {
                highlighter
                    .highlight(line, 0)
                    .buffer
                    .iter()
                    .map(|(style, text)| style.paint(text).to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        println!("no functions defined, you can define one like this:\n{highlighted}");
        return Ok(());
    }
    let mut table = NuTable::new(functions.len() + 1, 1);
    table.set_row(0, vec![NuRecordsValue::new("function name".into())]);
    for (i, name) in functions.iter().enumerate() {
        table.set_row(
            i + 1,
            vec![NuRecordsValue::new(
                display_engine.render_markdown_inline(format!("**{}**", name).as_str()),
            )],
        );
    }
    table.set_data_style(TextStyle::basic_left().style(Style::new().fg(Color::DarkGray)));
    table.set_header_style(TextStyle::basic_center().style(Style::new().fg(Color::Yellow).bold()));
    table.set_theme(TableTheme::rounded());
    table.set_structure(false, true, false);
    let width = crossterm::terminal::size()?.0 as usize;
    println!(
        "{}",
        table.draw(width).unwrap_or("[cannot fit]".to_string())
    );
    Ok(())
}

fn get_available_programs_table<D: DisplayEngine>(
    available_programs_info: &[EntryInfo],
    display_engine: &D,
) -> Result<String> {
    let mut table = NuTable::new(available_programs_info.len() + 1, 4);
    let headers = vec![
        NuRecordsValue::new("Shell Entry".into()),
        NuRecordsValue::new("Description".into()),
        NuRecordsValue::new("Version On Client".into()),
        NuRecordsValue::new("Program ID".into()),
    ];
    table.set_row(0, headers);
    for (i, program_info) in available_programs_info.iter().enumerate() {
        let program_id = program_info
            .program_id
            .map(|id| {
                if id > u32::MAX as u64 {
                    format!("0x{:x}", id)
                } else {
                    id.to_string()
                }
            })
            .unwrap_or_else(|| "N/A".to_string());
        let row = vec![
            NuRecordsValue::new(
                display_engine
                    .render_markdown_inline(format!("**{}**", program_info.name).as_str()),
            ),
            NuRecordsValue::new(
                display_engine.render_markdown_inline(program_info.short_description),
            ),
            NuRecordsValue::new(
                display_engine
                    .render_markdown_inline(format!("`{}`", program_info.version).as_str()),
            ),
            NuRecordsValue::new(
                display_engine.render_markdown_inline(format!("`{}`", program_id).as_str()),
            ),
        ];
        table.set_row(i + 1, row);
    }

    table.set_data_style(TextStyle::basic_left().style(Style::new().fg(Color::DarkGray)));
    table.set_header_style(TextStyle::basic_center().style(Style::new().fg(Color::Yellow).bold()));
    table.set_theme(TableTheme::rounded());
    table.set_structure(false, true, false);
    let width = crossterm::terminal::size()?.0 as usize;
    let table_str = table.draw(width).unwrap_or("[cannot fit]".to_string());
    Ok(table_str)
}

fn get_program_info_markdown<D: DisplayEngine>(
    program_name: &str,
    available_programs_info: &[EntryInfo],
    display_engine: &D,
) -> Result<Option<String>> {
    let program_info = available_programs_info
        .iter()
        .find(|p| p.name == program_name);

    let markdown = r#"# {name}
## Info:
Version On Client: `{version}`
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
        display_engine.render_markdown(formatted_markdown.as_str()),
    ))
}
