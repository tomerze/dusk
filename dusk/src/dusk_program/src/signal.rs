use embassy_sync::channel::DynamicReceiver;

#[non_exhaustive]
pub enum Signal {
    Reap,
    Terminate,
    Unknown(u64),
}

impl From<u64> for Signal {
    fn from(value: u64) -> Self {
        match value {
            8 => Signal::Reap,
            15 => Signal::Terminate,
            other => Signal::Unknown(other),
        }
    }
}

pub type SignalReceiver<'a> = DynamicReceiver<'a, Signal>;
