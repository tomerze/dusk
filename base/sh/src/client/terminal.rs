use std::cell::Cell;
use std::thread_local;

thread_local! {
    static READING: Cell<bool> = const { Cell::new(false) };
}

/// Claims the terminal for reading, or returns `None` if somebody already holds
/// it.
///
/// Taking the claim and finding out whether it was free are the same step, so
/// two callers cannot both decide the terminal is theirs. A prompt lets go of
/// the claim while it runs a command, which is what lets that command open a
/// prompt of its own.
pub fn try_read() -> Option<ReadingGuard> {
    READING.with(|reading| {
        if reading.get() {
            None
        } else {
            reading.set(true);
            Some(ReadingGuard)
        }
    })
}

pub struct ReadingGuard;

impl Drop for ReadingGuard {
    fn drop(&mut self) {
        READING.with(|reading| reading.set(false));
    }
}
