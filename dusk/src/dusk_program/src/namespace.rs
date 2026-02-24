use crate::process::Process;
use crate::ready::Ready;
use crate::signal;
use alloc::boxed::Box;
use alloc::rc::Rc;
use dusk_capnp::GIT_REV;
use dusk_capnp::capnp_rpc::CapabilityServerSet;
use dusk_capnp::dusk_capnp::process;
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use hashbrown::HashMap;
use nohash_hasher::BuildNoHashHasher;
use tracing::info;

pub type SignalChannel = Channel<NoopRawMutex, signal::Signal, 8>;

pub type PsCapabilityServerSet = CapabilityServerSet<Box<dyn Process>, process::Client>;
pub type PsMap = HashMap<u64, Box<dyn Process>, BuildNoHashHasher<u64>>;
pub type PsSignalChannelMap = HashMap<u64, Rc<SignalChannel>, BuildNoHashHasher<u64>>;
pub type PsReadyMap = HashMap<u64, Ready, BuildNoHashHasher<u64>>;

/// A namespace is a container for processes and potentially other driver resources.
///
/// It is not `Send` or `Sync` and is intended to be used within a single thread or executor context.
/// The mutexes are used to allow interior mutability.
pub struct Namespace {
    pub id: u64,
    pub spawner: Spawner,
    pub ps_server_set: Mutex<CriticalSectionRawMutex, PsCapabilityServerSet>,
    pub ps_map: Mutex<CriticalSectionRawMutex, PsMap>,
    pub ps_signal_channel_map: Mutex<CriticalSectionRawMutex, PsSignalChannelMap>,
    pub ps_ready_map: Mutex<CriticalSectionRawMutex, PsReadyMap>,
}

impl Namespace {
    pub fn new(id: u64, spawner: Spawner) -> Self {
        info!(
            namespace_id = id,
            version = dusk_capnp::VERSION,
            git_rev = GIT_REV,
            "namespace created"
        );
        let ps_server_set = Mutex::<
            CriticalSectionRawMutex,
            CapabilityServerSet<Box<dyn Process>, process::Client>,
        >::new(CapabilityServerSet::new());

        let ps_map = Mutex::<CriticalSectionRawMutex, PsMap>::new(HashMap::default());
        let ps_signal_channel_map =
            Mutex::<CriticalSectionRawMutex, PsSignalChannelMap>::new(HashMap::default());
        let ps_ready_map = Mutex::<CriticalSectionRawMutex, PsReadyMap>::new(HashMap::default());

        Namespace {
            id,
            spawner,
            ps_server_set,
            ps_map,
            ps_signal_channel_map,
            ps_ready_map,
        }
    }
}
