use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, StyledText};

pub struct CustomHighlighter {
    pub external_commands: Vec<String>,
}

impl CustomHighlighter {
    fn flush_word(
        styled: &mut StyledText,
        current: &mut String,
        at_command_start: &mut bool,
        commands: &[String],
    ) {
        if current.is_empty() {
            return;
        }

        if *at_command_start && commands.iter().any(|c| c == current) {
            styled.push((Style::new().bold().fg(Color::Cyan), current.clone()));
        } else {
            styled.push((Style::new().fg(Color::White), current.clone()));
        }

        *at_command_start = false;
        current.clear();
    }
}
impl Highlighter for CustomHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut styled = StyledText::new();
        let mut chars = line.chars().peekable();

        let mut current = String::new();
        let mut in_single = false;
        let mut in_double = false;
        let mut escape = false;
        let mut at_command_start = true;

        while let Some(c) = chars.next() {
            if in_single {
                current.push(c);

                if escape {
                    escape = false;
                    continue;
                }

                if c == '\\' {
                    escape = true;
                    continue;
                }

                if c == '\'' {
                    styled.push((Style::new().fg(Color::Green), current.clone()));
                    current.clear();
                    in_single = false;
                }

                continue;
            }

            if in_double {
                current.push(c);

                if escape {
                    escape = false;
                    continue;
                }

                if c == '\\' {
                    escape = true;
                    continue;
                }

                if c == '"' {
                    styled.push((Style::new().fg(Color::Green), current.clone()));
                    current.clear();
                    in_double = false;
                }

                continue;
            }

            match c {
                '\'' => {
                    Self::flush_word(
                        &mut styled,
                        &mut current,
                        &mut at_command_start,
                        &self.external_commands,
                    );
                    in_single = true;
                    current.push(c);
                }

                '"' => {
                    Self::flush_word(
                        &mut styled,
                        &mut current,
                        &mut at_command_start,
                        &self.external_commands,
                    );
                    in_double = true;
                    current.push(c);
                }

                '&' => {
                    Self::flush_word(
                        &mut styled,
                        &mut current,
                        &mut at_command_start,
                        &self.external_commands,
                    );

                    if chars.peek() == Some(&'&') {
                        chars.next();
                        styled.push((Style::new().bold().fg(Color::Magenta), "&&".into()));
                    } else {
                        styled.push((Style::new().bold().fg(Color::Yellow), "&".into()));
                    }

                    at_command_start = true;
                }

                '|' => {
                    Self::flush_word(
                        &mut styled,
                        &mut current,
                        &mut at_command_start,
                        &self.external_commands,
                    );

                    if chars.peek() == Some(&'|') {
                        chars.next();
                        styled.push((Style::new().bold().fg(Color::Magenta), "||".into()));
                    } else {
                        styled.push((Style::new().bold().fg(Color::White), "|".into()));
                    }

                    at_command_start = true;
                }

                ';' => {
                    Self::flush_word(
                        &mut styled,
                        &mut current,
                        &mut at_command_start,
                        &self.external_commands,
                    );
                    styled.push((Style::new().bold().fg(Color::Yellow), ";".into()));
                    at_command_start = true;
                }

                ' ' | '\t' => {
                    Self::flush_word(
                        &mut styled,
                        &mut current,
                        &mut at_command_start,
                        &self.external_commands,
                    );
                    styled.push((Style::new().fg(Color::White), c.to_string()));
                }

                _ => {
                    current.push(c);
                }
            }
        }

        Self::flush_word(
            &mut styled,
            &mut current,
            &mut at_command_start,
            &self.external_commands,
        );

        styled
    }
}
