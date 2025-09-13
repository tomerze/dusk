use crate::process::Process;
use crate::signal;
use alloc::boxed::Box;
use alloc::rc::Rc;
use capnp_rpc::CapabilityServerSet;
use dusk_capnp::dusk_capnp::process;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use hashbrown::HashMap;
use log::info;
use nohash_hasher::BuildNoHashHasher;

pub type SignalChannel = Channel<NoopRawMutex, signal::Signal, 8>;
pub type PsCapabilityServerSet = CapabilityServerSet<Box<dyn Process>, process::Client>;
pub type PsMap = HashMap<u64, Box<dyn Process>, BuildNoHashHasher<u64>>;
pub type PsSignalChannelMap = HashMap<u64, Rc<SignalChannel>, BuildNoHashHasher<u64>>;

/// A namespace is a container for processes and potentially other driver resources.
///
/// It is not `Send` or `Sync` and is intended to be used within a single thread or executor context.
/// The mutexes are used to allow interior mutability.
pub struct Namespace {
    pub id: u64,
    pub ps_server_set: Mutex<NoopRawMutex, PsCapabilityServerSet>,
    pub ps_map: Mutex<NoopRawMutex, PsMap>,
    pub ps_signal_channel_map: Mutex<NoopRawMutex, PsSignalChannelMap>,
}

impl Namespace {
    pub fn new(id: u64) -> Self {
        info!("namespace `{}` created", id);
        let ps_server_set = Mutex::<
            NoopRawMutex,
            CapabilityServerSet<Box<dyn Process>, process::Client>,
        >::new(CapabilityServerSet::new());

        let ps_map = Mutex::<NoopRawMutex, PsMap>::new(HashMap::default());
        let ps_signal_channel_map =
            Mutex::<NoopRawMutex, PsSignalChannelMap>::new(HashMap::default());

        Namespace {
            id,
            ps_server_set,
            ps_map,
            ps_signal_channel_map,
        }
    }
}
