pub enum Signal {
    Terminate,
    Kill,
    Unknown(u64),
}

impl From<u64> for Signal {
    fn from(value: u64) -> Self {
        match value {
            9 => Signal::Kill,
            15 => Signal::Terminate,
            other => Signal::Unknown(other),
        }
    }
}
