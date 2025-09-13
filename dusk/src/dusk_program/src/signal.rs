pub enum Signal {
    Terminate,
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
