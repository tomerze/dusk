use alloc::rc::Rc;
use embassy_sync::channel::DynamicReceiver;

use crate::program_args::ProgramArgs;

#[non_exhaustive]
pub enum Signal {
    Terminate,
    /// The process was created a second time from args that fix its pid, and
    /// those args are carried here.
    ///
    /// Ignore it. Fixing a pid means the running process is the answer and the
    /// second set of args is discarded; this is the escape hatch for the rare
    /// process that cannot let them go.
    Rerun(Rc<ProgramArgs>),
    Unknown(u64),
}

impl From<u64> for Signal {
    fn from(value: u64) -> Self {
        match value {
            15 => Signal::Terminate,
            other => Signal::Unknown(other),
        }
    }
}

pub type SignalReceiver<'a> = DynamicReceiver<'a, Signal>;
