//! The non-interactive log dump: `logs --replay-only` with no url prints the
//! buffered signals to stdout, one per line, and exits — the scriptable
//! counterpart to the interactive [`viewer`](super::viewer), whose line
//! rendering it reuses.

use super::acknowledge;
use super::viewer::entry_lines;
use crate::logs_capnp::logs_args;
use capnp::capability::Promise;
use std::io::Write as _;

/// Prints each streamed signal to stdout as a plain line. The node finishes a
/// `--replay-only` stream on its own once the history is drained, so this never
/// needs to signal stop.
pub struct PrintStream;

impl PrintStream {
    pub fn new() -> Self {
        PrintStream
    }
}

impl Default for PrintStream {
    fn default() -> Self {
        Self::new()
    }
}

impl logs_args::server::Server for PrintStream {
    fn send(&mut self, params: logs_args::server::SendParams) -> Promise<(), capnp::Error> {
        let signal_batch = match params.get().and_then(|params| params.get_signal_batch()) {
            Ok(signal_batch) => signal_batch,
            Err(error) => return Promise::err(error),
        };
        let entries = match signal_batch.get_signals() {
            Ok(entries) => entries,
            Err(error) => return Promise::err(error),
        };
        let ack = match signal_batch.get_ack() {
            Ok(ack) => ack,
            Err(error) => return Promise::err(error),
        };
        let mut stdout = std::io::stdout().lock();
        for line in entry_lines(entries) {
            // A closed stdout (e.g. piped into `head`) just ends the dump.
            let _ = writeln!(stdout, "{}", line.plain);
        }
        Promise::from_future(async move {
            acknowledge(ack).await;
            Ok(())
        })
    }

    fn stop(
        &mut self,
        _params: logs_args::server::StopParams,
        _results: logs_args::server::StopResults,
    ) -> Promise<(), capnp::Error> {
        Promise::from_future(std::future::pending())
    }
}
