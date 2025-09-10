use capnp_rpc::CapabilityServerSet;
use dusk_program::Process;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use log::info;

pub struct Namespace {
    pub id: u64,
    pub processes: Mutex<
        CriticalSectionRawMutex,
        CapabilityServerSet<Box<dyn Process>, dusk_capnp::dusk_capnp::process::Client>,
    >,
}

impl Namespace {
    pub fn new(id: u64) -> Self {
        info!("namespace `{}` created", id);
        let processes = Mutex::<
            CriticalSectionRawMutex,
            CapabilityServerSet<Box<dyn Process>, dusk_capnp::dusk_capnp::process::Client>,
        >::new(CapabilityServerSet::new());
        Namespace { id, processes }
    }
}
