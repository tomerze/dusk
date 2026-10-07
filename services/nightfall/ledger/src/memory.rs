use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::log::{LedgerLog, LogError, LogErrorKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    Begin(LogErrorKind),
    Append(LogErrorKind),
    Commit(LogErrorKind),
    CommitAppliedThen(LogErrorKind),
    Abort(LogErrorKind),
    ReadTail(LogErrorKind),
    Recover(LogErrorKind),
}

#[derive(Default)]
struct MemoryState {
    committed: Vec<Vec<u8>>,
    transaction: Option<Vec<Vec<u8>>>,
    faults: VecDeque<Fault>,
    commits: usize,
    recoveries: usize,
    broken: bool,
}

#[derive(Clone, Default)]
pub struct MemoryLog {
    state: Arc<Mutex<MemoryState>>,
}

impl MemoryLog {
    pub fn new() -> MemoryLog {
        MemoryLog::default()
    }

    pub fn with_records(records: Vec<Vec<u8>>) -> MemoryLog {
        let log = MemoryLog::default();
        log.lock().committed = records;
        log
    }

    pub fn records(&self) -> Vec<Vec<u8>> {
        self.lock().committed.clone()
    }

    pub fn commits(&self) -> usize {
        self.lock().commits
    }

    pub fn recoveries(&self) -> usize {
        self.lock().recoveries
    }

    pub fn inject(&self, fault: Fault) {
        self.lock().faults.push_back(fault);
    }

    fn lock(&self) -> MutexGuard<'_, MemoryState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn take_fault(
    state: &mut MemoryState,
    matches: impl Fn(&Fault) -> Option<LogErrorKind>,
) -> Option<LogErrorKind> {
    let kind = state.faults.front().and_then(&matches)?;
    state.faults.pop_front();
    Some(kind)
}

fn failure(state: &mut MemoryState, kind: LogErrorKind, operation: &str) -> LogError {
    if matches!(kind, LogErrorKind::Fatal | LogErrorKind::Fenced) {
        state.broken = true;
    }
    LogError::new(kind, format!("injected {operation} failure"))
}

impl LedgerLog for MemoryLog {
    fn begin(&mut self) -> Result<(), LogError> {
        let mut state = self.lock();
        if state.broken {
            return Err(LogError::new(
                LogErrorKind::Fatal,
                "producer needs recovery",
            ));
        }
        if let Some(kind) = take_fault(&mut state, |fault| match fault {
            Fault::Begin(kind) => Some(*kind),
            _ => None,
        }) {
            return Err(failure(&mut state, kind, "begin"));
        }
        if state.transaction.is_some() {
            return Err(LogError::new(
                LogErrorKind::Fatal,
                "a transaction is already open",
            ));
        }
        state.transaction = Some(Vec::new());
        Ok(())
    }

    fn append(&mut self, payload: &[u8]) -> Result<(), LogError> {
        let mut state = self.lock();
        if let Some(kind) = take_fault(&mut state, |fault| match fault {
            Fault::Append(kind) => Some(*kind),
            _ => None,
        }) {
            return Err(failure(&mut state, kind, "append"));
        }
        match state.transaction.as_mut() {
            Some(transaction) => {
                transaction.push(payload.to_vec());
                Ok(())
            }
            None => Err(LogError::new(
                LogErrorKind::Fatal,
                "append outside a transaction",
            )),
        }
    }

    fn commit(&mut self) -> Result<(), LogError> {
        let mut state = self.lock();
        let fault = state.faults.front().copied();
        let transaction = state
            .transaction
            .take()
            .ok_or_else(|| LogError::new(LogErrorKind::Fatal, "commit outside a transaction"))?;
        match fault {
            Some(Fault::Commit(kind)) => {
                state.faults.pop_front();
                if kind != LogErrorKind::Fatal && kind != LogErrorKind::Fenced {
                    state.transaction = Some(transaction);
                }
                Err(failure(&mut state, kind, "commit"))
            }
            Some(Fault::CommitAppliedThen(kind)) => {
                state.faults.pop_front();
                state.committed.extend(transaction);
                state.commits += 1;
                Err(failure(&mut state, kind, "commit"))
            }
            _ => {
                state.committed.extend(transaction);
                state.commits += 1;
                Ok(())
            }
        }
    }

    fn abort(&mut self) -> Result<(), LogError> {
        let mut state = self.lock();
        if let Some(kind) = take_fault(&mut state, |fault| match fault {
            Fault::Abort(kind) => Some(*kind),
            _ => None,
        }) {
            return Err(failure(&mut state, kind, "abort"));
        }
        state.transaction = None;
        Ok(())
    }

    fn read_tail(&mut self, count: usize) -> Result<Vec<Vec<u8>>, LogError> {
        let mut state = self.lock();
        if let Some(kind) = take_fault(&mut state, |fault| match fault {
            Fault::ReadTail(kind) => Some(*kind),
            _ => None,
        }) {
            return Err(failure(&mut state, kind, "read_tail"));
        }
        let start = state.committed.len().saturating_sub(count);
        Ok(state.committed[start..].to_vec())
    }

    fn recover(&mut self) -> Result<(), LogError> {
        let mut state = self.lock();
        if let Some(kind) = take_fault(&mut state, |fault| match fault {
            Fault::Recover(kind) => Some(*kind),
            _ => None,
        }) {
            return Err(failure(&mut state, kind, "recover"));
        }
        state.transaction = None;
        state.broken = false;
        state.recoveries += 1;
        Ok(())
    }
}
