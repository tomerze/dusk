use alloc::rc::Rc;
use embassy_sync::channel::DynamicReceiver;

use crate::program_args::ProgramArgs;

#[non_exhaustive]
pub enum Signal {
    Sweep,
    Reap,
    Terminate,
    Rerun(Rc<ProgramArgs>),
    Unknown(u64),
}

impl From<u64> for Signal {
    fn from(value: u64) -> Self {
        match value {
            7 => Signal::Sweep,
            8 => Signal::Reap,
            15 => Signal::Terminate,
            other => Signal::Unknown(other),
        }
    }
}

pub type SignalReceiver<'a> = DynamicReceiver<'a, Signal>;
