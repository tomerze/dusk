use std::io::{Write as _, stdout};
use std::panic;
use std::string::String;
use std::write;

use nu_ansi_term::{Color, Style};

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const TICK: std::time::Duration = std::time::Duration::from_millis(100);

fn paint(frame: &str, label: &str) {
    let mut handle = stdout().lock();
    let _ = write!(
        handle,
        "\r{} {}\x1b[K",
        Style::new().fg(Color::Yellow).bold().paint(frame),
        Style::new().fg(Color::DarkGray).paint(label),
    );
    let _ = handle.flush();
}

/// Erase the last spinner line painted (resets to column 0 + clear-to-EOL).
fn clear() {
    let mut handle = stdout().lock();
    let _ = write!(handle, "\r\x1b[K");
    let _ = handle.flush();
}

/// Drive a braille spinner alongside `future`. `label_for_frame` is
/// invoked on every tick so the caller can show changing state (a
/// running token count, a status message, …) instead of a static label.
/// The spinner is erased before the future's result is returned.
pub async fn with_spinner<F, T, L>(future: F, mut label_for_frame: L) -> T
where
    F: std::future::Future<Output = T>,
    L: FnMut() -> String,
{
    let mut interval = tokio::time::interval(TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut frame_index = 0usize;
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            result = &mut future => {
                clear();
                return result;
            }
            _ = interval.tick() => {
                paint(
                    FRAMES[frame_index % FRAMES.len()],
                    &label_for_frame(),
                );
                frame_index = frame_index.wrapping_add(1);
            }
        }
    }
}
