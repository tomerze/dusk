#[derive(Debug, Clone, PartialEq)]
pub struct SyntaxError;

impl core::fmt::Display for SyntaxError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("syntax error")
    }
}

impl core::error::Error for SyntaxError {}
