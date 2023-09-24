use crate::prompt;
use anyhow::Result;
use chrono::Duration;
use crossterm::{event::DisableBracketedPaste, execute};
use reedline::{Reedline, Signal};
use std::io::stdout;
use tracing::info;

pub struct Shell {
    hostname: String,
    available_programs: Vec<String>,
}

impl Shell {
    pub fn new(hostname: impl Into<String>, mut available_programs: Vec<String>) -> Self {
        let mut builtins = vec!["clear".into(), "exit".into()];
        available_programs.append(&mut builtins);
        Shell {
            hostname: hostname.into(),
            available_programs,
        }
    }

    pub async fn run(self) -> Result<()> {
        let mut line_editor = prompt::get_line_editor(self.available_programs)?;

        let prompt = prompt::DuskPrompt::new(&self.hostname);

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

                    Self::process_line(&buffer, &mut line_editor).await?;

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

    async fn process_line(line: &str, line_editor: &mut Reedline) -> Result<()> {
        let mut args = line.split_ascii_whitespace();

        match args.next() {
            Some("exit") => {
                std::process::exit(0);
            }
            Some("clear") => {
                line_editor.clear_screen()?;
            }
            Some(program) => {
                Self::process_command(program, args.collect()).await;
            }
            None => {}
        };

        Ok(())
    }

    async fn process_command(program: &str, args: Vec<&str>) {
        info!("run program {program} with args {args:?}");
    }
}
