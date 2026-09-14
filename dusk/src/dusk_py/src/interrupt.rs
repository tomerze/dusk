use dusk_program::anyhow::{Result, anyhow};
use dusk_program_sh::client::stop::stop_innermost;
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

static PROMPT_OPEN: AtomicBool = AtomicBool::new(false);

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

const INTERRUPT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

extern "C" fn note_interrupt(_signal: i32) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

pub(crate) async fn with_interrupts<F: Future<Output = Result<()>>>(prompt: F) -> Result<()> {
    if PROMPT_OPEN
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(anyhow!("there is already an open prompt in this process"));
    }
    let dusk_handler = SigAction::new(
        SigHandler::Handler(note_interrupt),
        SaFlags::SA_RESTART,
        SigSet::empty(),
    );
    INTERRUPTED.store(false, Ordering::SeqCst);
    let python_handler = match unsafe { sigaction(Signal::SIGINT, &dusk_handler) } {
        Ok(python_handler) => Some(python_handler),
        Err(error) => {
            tracing::warn!(error = %error, "couldn't take ctrl+c for the prompt");
            None
        }
    };
    let watcher = tokio::task::spawn_local(async move {
        loop {
            tokio::time::sleep(INTERRUPT_POLL_INTERVAL).await;
            if INTERRUPTED.swap(false, Ordering::SeqCst) {
                stop_innermost();
            }
        }
    });
    let result = prompt.await;
    watcher.abort();
    if let Some(python_handler) = python_handler
        && let Err(error) = unsafe { sigaction(Signal::SIGINT, &python_handler) }
    {
        tracing::warn!(error = %error, "couldn't give ctrl+c back to python");
    }
    PROMPT_OPEN.store(false, Ordering::SeqCst);
    result
}
