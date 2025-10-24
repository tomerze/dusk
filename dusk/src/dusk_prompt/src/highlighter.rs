use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, StyledText};

pub struct CustomHighlighter {
    pub external_commands: Vec<String>,
}

impl Highlighter for CustomHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut styled_text = StyledText::new();

        if self
            .external_commands
            .clone()
            .iter()
            .any(|x| line.contains(x))
        {
            let matches: Vec<&str> = self
                .external_commands
                .iter()
                .filter(|c| line.contains(*c))
                .map(std::ops::Deref::deref)
                .collect();
            let longest_match = matches.iter().fold("".to_string(), |acc, &item| {
                if item.len() > acc.len() {
                    item.to_string()
                } else {
                    acc
                }
            });
            let buffer_split: Vec<&str> = line.splitn(2, &longest_match).collect();

            styled_text.push((Style::new().fg(Color::White), buffer_split[0].to_string()));
            styled_text.push((Style::new().bold().fg(Color::Cyan), longest_match));
            styled_text.push((Style::new().fg(Color::White), buffer_split[1].to_string()));
        } else if self.external_commands.is_empty() {
            styled_text.push((Style::new().fg(Color::Cyan), line.to_string()));
        } else {
            styled_text.push((Style::new().fg(Color::Red), line.to_string()));
        }

        styled_text
    }
}
