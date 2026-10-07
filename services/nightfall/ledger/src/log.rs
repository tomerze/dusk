#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogErrorKind {
    Retriable,
    Abortable,
    Fatal,
    Fenced,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct LogError {
    pub kind: LogErrorKind,
    pub message: String,
}

impl LogError {
    pub fn new(kind: LogErrorKind, message: impl Into<String>) -> LogError {
        LogError {
            kind,
            message: message.into(),
        }
    }
}

pub trait LedgerLog: Send {
    fn begin(&mut self) -> Result<(), LogError>;
    fn append(&mut self, payload: &[u8]) -> Result<(), LogError>;
    fn commit(&mut self) -> Result<(), LogError>;
    fn abort(&mut self) -> Result<(), LogError>;
    fn read_tail(&mut self, count: usize) -> Result<Vec<Vec<u8>>, LogError>;
    fn recover(&mut self) -> Result<(), LogError>;
}
