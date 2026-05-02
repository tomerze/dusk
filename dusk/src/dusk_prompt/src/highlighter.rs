use std::sync::{Arc, Mutex};

use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, StyledText};

/// Live, externally-mutable list of currently defined function names.
pub type FunctionNames = Arc<Mutex<Vec<String>>>;

pub struct CustomHighlighter {
    pub external_commands: Vec<String>,
    pub functions: FunctionNames,
}

fn function_call_style() -> Style {
    Style::new().bold().fg(Color::LightBlue)
}

fn function_def_style() -> Style {
    Style::new().fg(Color::LightBlue)
}

fn bracket_style() -> Style {
    Style::new().fg(Color::Yellow)
}

impl CustomHighlighter {
    /// Push the currently-accumulated word with a colour decided by where we
    /// are. Returns `Some(index_in_buffer)` when the word was pushed at a
    /// command-start position — caller may want to recolour it as a function
    /// definition if a `(` follows.
    fn flush_word(
        styled: &mut StyledText,
        current: &mut String,
        at_command_start: &mut bool,
        commands: &[String],
        functions: &[String],
        local_functions: &[String],
    ) -> Option<usize> {
        if current.is_empty() {
            return None;
        }
        let was_at_start = *at_command_start;
        let is_function = was_at_start
            && (functions.iter().any(|f| f == current)
                || local_functions.iter().any(|f| f == current));
        let is_program = was_at_start && commands.iter().any(|c| c == current);
        if is_function {
            styled.push((function_call_style(), current.clone()));
        } else if is_program {
            styled.push((Style::new().bold().fg(Color::Cyan), current.clone()));
        } else {
            styled.push((Style::new().fg(Color::White), current.clone()));
        }
        *at_command_start = false;
        let idx = styled.buffer.len() - 1;
        current.clear();
        if was_at_start { Some(idx) } else { None }
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
        let functions = self.functions.lock().unwrap();
        // Function names defined earlier on this same line. Lets the body of a
        // brand-new `foo() { foo }` highlight the inner `foo` as a call before
        // it has propagated to the live function table.
        let mut local_functions: Vec<String> = Vec::new();
        // Index in `styled.buffer` of the most recently flushed command-start
        // word — a candidate for being recoloured as a function-def name if a
        // `(` follows (allowing whitespace between).
        let mut def_candidate_idx: Option<usize> = None;

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
                    def_candidate_idx = None;
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
                    def_candidate_idx = None;
                }

                continue;
            }

            match c {
                '/' if chars.peek() == Some(&'/') => {
                    Self::flush_word(
                        &mut styled,
                        &mut current,
                        &mut at_command_start,
                        &self.external_commands,
                        &functions,
                        &local_functions,
                    );
                    chars.next(); // consume the second '/'
                    let mut comment = String::from("//");
                    while let Some(&next) = chars.peek() {
                        if next == '\n' {
                            break;
                        }
                        comment.push(next);
                        chars.next();
                    }
                    styled.push((Style::new().fg(Color::DarkGray), comment));
                    def_candidate_idx = None;
                }

                '/' if chars.peek() == Some(&'*') => {
                    Self::flush_word(
                        &mut styled,
                        &mut current,
                        &mut at_command_start,
                        &self.external_commands,
                        &functions,
                        &local_functions,
                    );
                    chars.next(); // consume the '*'
                    let mut comment = String::from("/*");
                    let mut prev = '\0';
                    while let Some(next) = chars.next() {
                        comment.push(next);
                        if prev == '*' && next == '/' {
                            break;
                        }
                        prev = next;
                    }
                    styled.push((Style::new().fg(Color::DarkGray), comment));
                    def_candidate_idx = None;
                }

                '\'' => {
                    Self::flush_word(
                        &mut styled,
                        &mut current,
                        &mut at_command_start,
                        &self.external_commands,
                        &functions,
                        &local_functions,
                    );
                    in_single = true;
                    current.push(c);
                    def_candidate_idx = None;
                }

                '"' => {
                    Self::flush_word(
                        &mut styled,
                        &mut current,
                        &mut at_command_start,
                        &self.external_commands,
                        &functions,
                        &local_functions,
                    );
                    in_double = true;
                    current.push(c);
                    def_candidate_idx = None;
                }

                '&' => {
                    Self::flush_word(
                        &mut styled,
                        &mut current,
                        &mut at_command_start,
                        &self.external_commands,
                        &functions,
                        &local_functions,
                    );

                    if chars.peek() == Some(&'&') {
                        chars.next();
                        styled.push((Style::new().bold().fg(Color::Magenta), "&&".into()));
                    } else {
                        styled.push((Style::new().fg(Color::White), "&".into()));
                    }

                    at_command_start = true;
                    def_candidate_idx = None;
                }

                '|' => {
                    Self::flush_word(
                        &mut styled,
                        &mut current,
                        &mut at_command_start,
                        &self.external_commands,
                        &functions,
                        &local_functions,
                    );

                    if chars.peek() == Some(&'|') {
                        chars.next();
                        styled.push((Style::new().bold().fg(Color::Magenta), "||".into()));
                    } else {
                        styled.push((Style::new().bold().fg(Color::White), "|".into()));
                    }

                    at_command_start = true;
                    def_candidate_idx = None;
                }

                ';' => {
                    Self::flush_word(
                        &mut styled,
                        &mut current,
                        &mut at_command_start,
                        &self.external_commands,
                        &functions,
                        &local_functions,
                    );
                    styled.push((Style::new().bold().fg(Color::Yellow), ";".into()));
                    at_command_start = true;
                    def_candidate_idx = None;
                }

                '(' => {
                    let def_name: Option<String>;
                    if at_command_start && !current.is_empty() {
                        // `name(` with no space — current word is the def name.
                        let name = current.clone();
                        styled.push((function_def_style(), name.clone()));
                        current.clear();
                        at_command_start = false;
                        def_name = Some(name);
                    } else {
                        Self::flush_word(
                            &mut styled,
                            &mut current,
                            &mut at_command_start,
                            &self.external_commands,
                            &functions,
                            &local_functions,
                        );
                        if let Some(idx) = def_candidate_idx {
                            // `name (` with whitespace between — recolour the
                            // word that was already pushed.
                            let name = styled.buffer[idx].1.clone();
                            styled.buffer[idx].0 = function_def_style();
                            def_name = Some(name);
                        } else {
                            def_name = None;
                        }
                    }
                    if let Some(name) = def_name {
                        local_functions.push(name);
                    }
                    styled.push((bracket_style(), "(".into()));
                    def_candidate_idx = None;
                }

                ')' => {
                    Self::flush_word(
                        &mut styled,
                        &mut current,
                        &mut at_command_start,
                        &self.external_commands,
                        &functions,
                        &local_functions,
                    );
                    styled.push((bracket_style(), ")".into()));
                    def_candidate_idx = None;
                }

                '{' => {
                    Self::flush_word(
                        &mut styled,
                        &mut current,
                        &mut at_command_start,
                        &self.external_commands,
                        &functions,
                        &local_functions,
                    );
                    styled.push((bracket_style().bold(), "{".into()));
                    at_command_start = true;
                    def_candidate_idx = None;
                }

                '}' => {
                    Self::flush_word(
                        &mut styled,
                        &mut current,
                        &mut at_command_start,
                        &self.external_commands,
                        &functions,
                        &local_functions,
                    );
                    styled.push((bracket_style().bold(), "}".into()));
                    at_command_start = true;
                    def_candidate_idx = None;
                }

                ' ' | '\t' => {
                    let pushed = Self::flush_word(
                        &mut styled,
                        &mut current,
                        &mut at_command_start,
                        &self.external_commands,
                        &functions,
                        &local_functions,
                    );
                    if pushed.is_some() {
                        // Word was at command-start; remember it in case a `(`
                        // follows after this whitespace.
                        def_candidate_idx = pushed;
                    }
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
            &functions,
            &local_functions,
        );

        styled
    }
}
