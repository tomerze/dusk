use dusk_program::anyhow::{Result, anyhow};
use dusk_program_sh::client::stop::stop_innermost;
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

static PROMPT_OPEN: AtomicBool = AtomicBool::new(false);

static CTRL_C_PRESSED: AtomicBool = AtomicBool::new(false);

const CTRL_C_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

extern "C" fn record_ctrl_c(_signal: i32) {
    CTRL_C_PRESSED.store(true, Ordering::SeqCst);
}

pub(crate) async fn stop_on_ctrl_c<F: Future<Output = Result<()>>>(prompt: F) -> Result<()> {
    if PROMPT_OPEN
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(anyhow!("there is already an open prompt in this process"));
    }
    let recording_handler = SigAction::new(
        SigHandler::Handler(record_ctrl_c),
        SaFlags::SA_RESTART,
        SigSet::empty(),
    );
    CTRL_C_PRESSED.store(false, Ordering::SeqCst);
    let python_handler = match unsafe { sigaction(Signal::SIGINT, &recording_handler) } {
        Ok(python_handler) => Some(python_handler),
        Err(error) => {
            tracing::warn!(error = %error, "couldn't take ctrl+c for the prompt");
            None
        }
    };
    let watcher = tokio::task::spawn_local(async move {
        loop {
            tokio::time::sleep(CTRL_C_POLL_INTERVAL).await;
            if CTRL_C_PRESSED.swap(false, Ordering::SeqCst) {
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
