use alloc::string::{String, ToString};

#[derive(Debug, Clone, PartialEq)]
pub struct SyntaxError {
    stopped_at: Option<String>,
}

impl SyntaxError {
    pub(crate) fn whole_script() -> Self {
        Self { stopped_at: None }
    }

    pub(crate) fn at(remaining: &str) -> Self {
        let line = remaining.lines().next().unwrap_or(remaining).trim();
        Self {
            stopped_at: Some(line.to_string()),
        }
    }

    pub fn stopped_at(&self) -> Option<&str> {
        self.stopped_at.as_deref()
    }
}

impl core::fmt::Display for SyntaxError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match &self.stopped_at {
            Some(stopped_at) => formatter.write_fmt(format_args!("syntax error at `{stopped_at}`")),
            None => formatter.write_str("syntax error"),
        }
    }
}

impl core::error::Error for SyntaxError {}
