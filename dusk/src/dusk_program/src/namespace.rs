use crate::process::Process;
use alloc::boxed::Box;
use capnp_rpc::CapabilityServerSet;
use dusk_capnp::dusk_capnp::process;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex;
use hashbrown::HashMap;
use log::info;
use nohash_hasher::BuildNoHashHasher;

pub type ProcessMap = HashMap<u64, Box<dyn Process>, BuildNoHashHasher<u64>>;

/// A namespace is a container for processes and potentially other driver resources.
///
/// It is not `Send` or `Sync` and is intended to be used within a single thread or executor context.
pub struct Namespace {
    pub id: u64,
    pub ps_server_set: Mutex<NoopRawMutex, CapabilityServerSet<Box<dyn Process>, process::Client>>,
    pub ps_map: Mutex<NoopRawMutex, ProcessMap>,
}

impl Namespace {
    pub fn new(id: u64) -> Self {
        info!("namespace `{}` created", id);
        let ps_server_set = Mutex::<
            NoopRawMutex,
            CapabilityServerSet<Box<dyn Process>, process::Client>,
        >::new(CapabilityServerSet::new());

        let ps_map = Mutex::<NoopRawMutex, ProcessMap>::new(HashMap::default());

        Namespace {
            id,
            ps_server_set,
            ps_map,
        }
    }
}
