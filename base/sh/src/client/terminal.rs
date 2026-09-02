use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
use std::thread_local;
use std::vec::Vec;
use tokio::sync::Notify;

thread_local! {
    static READING: Cell<bool> = const { Cell::new(false) };
    static OPEN_PROMPTS: Cell<usize> = const { Cell::new(0) };
    static DRIVEN: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static CLOSED: Rc<Notify> = Rc::new(Notify::new());
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

/// Counts a prompt as open on this terminal until the returned guard is
/// dropped.
///
/// Take it before the prompt's task is spawned, not inside it: between the two
/// there is a window in which the count reads as nothing open and whoever is
/// waiting on it carries on.
pub fn open_prompt() -> PromptGuard {
    OPEN_PROMPTS.with(|count| count.set(count.get() + 1));
    PromptGuard
}

/// How many prompts are open on this terminal.
pub fn open_prompts() -> usize {
    OPEN_PROMPTS.with(|count| count.get())
}

pub struct PromptGuard;

impl Drop for PromptGuard {
    fn drop(&mut self) {
        OPEN_PROMPTS.with(|count| count.set(count.get().saturating_sub(1)));
        CLOSED.with(|closed| closed.notify_waiters());
    }
}

/// Waits until the prompts open on this terminal are back down to `count` —
/// every prompt opened since has closed.
pub async fn wait_until_prompts_closed(count: usize) {
    loop {
        let closed = CLOSED.with(|closed| closed.clone());
        let notified = closed.notified();
        if open_prompts() <= count {
            return;
        }
        notified.await;
    }
}

/// Records that this client drives the process at `pid` until the returned
/// guard is dropped.
///
/// Take it before the process is created, not after: the node answers the
/// process's created callback while creating it, and a callback that arrives
/// before the pid is recorded opens a prompt this client did not ask for.
pub fn drive(pid: u64) -> DriveGuard {
    DRIVEN.with(|driven| driven.borrow_mut().push(pid));
    DriveGuard(pid)
}

/// Whether this client already drives the process at `pid`.
pub fn drives(pid: u64) -> bool {
    DRIVEN.with(|driven| driven.borrow().contains(&pid))
}

pub struct DriveGuard(u64);

impl Drop for DriveGuard {
    fn drop(&mut self) {
        DRIVEN.with(|driven| {
            let mut driven = driven.borrow_mut();
            if let Some(index) = driven.iter().position(|pid| *pid == self.0) {
                driven.remove(index);
            }
        });
    }
}
