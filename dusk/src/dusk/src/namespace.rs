use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use log::info;

pub struct Namespace {
    pub id: u64,
    pub processes: Mutex<CriticalSectionRawMutex, ()>,
}

impl Namespace {
    pub fn new(id: u64) -> Self {
        info!("namespace `{}` created", id);
        let processes = Mutex::<CriticalSectionRawMutex, ()>::new(());
        Namespace { id, processes }
    }
}
