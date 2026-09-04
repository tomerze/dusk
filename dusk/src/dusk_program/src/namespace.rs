use crate::launcher_set::LauncherSet;
use crate::process::Process;
use crate::process::ProcessContext;
use crate::program_args::ProgramArgs;
use crate::ready::Ready;
use crate::signal;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::sync::atomic::AtomicU64;
use dusk_capnp::GIT_REV;
use dusk_capnp::capnp_rpc::CapabilityServerSet;
use dusk_capnp::dusk_capnp::process;
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use hashbrown::HashMap;
use nohash_hasher::BuildNoHashHasher;
use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};
use sha2::{Digest, Sha256};

use tracing::info;
use tracing::warn;

pub type SignalChannel = Channel<NoopRawMutex, signal::Signal, 8>;

pub type PsCapabilityServerSet = CapabilityServerSet<Box<dyn Process>, process::Client>;
pub type PsMap = HashMap<u64, Box<dyn Process>, BuildNoHashHasher<u64>>;
pub type PsSignalChannelMap = HashMap<u64, Rc<SignalChannel>, BuildNoHashHasher<u64>>;
pub type PsReadyMap = HashMap<u64, Ready, BuildNoHashHasher<u64>>;
pub type ExitWatch = alloc::rc::Rc<
    embassy_sync::watch::Watch<
        CriticalSectionRawMutex,
        Option<Result<(), alloc::string::String>>,
        16,
    >,
>;
pub type PsExitMap = HashMap<u64, ExitWatch, BuildNoHashHasher<u64>>;

/// A namespace is a container for processes and potentially other driver resources.
///
/// It is not `Send` or `Sync` and is intended to be used within a single thread or executor context.
/// The mutexes are used to allow interior mutability.
pub struct Namespace {
    pub id: u64,                  // Random namespace id
    pub creation_time: AtomicU64, // Timestamp in which this namespace was created. Unix time in miliseconds.
    pub rng: Mutex<CriticalSectionRawMutex, ChaCha20Rng>,
    pub spawner: Spawner,
    pub ps_server_set: Mutex<CriticalSectionRawMutex, PsCapabilityServerSet>,
    pub ps_map: Mutex<CriticalSectionRawMutex, PsMap>,
    pub ps_signal_channel_map: Mutex<CriticalSectionRawMutex, PsSignalChannelMap>,
    pub ps_ready_map: Mutex<CriticalSectionRawMutex, PsReadyMap>,
    pub ps_exit_map: Mutex<CriticalSectionRawMutex, PsExitMap>,
}

impl Namespace {
    pub fn new(random_seed: u128, spawner: Spawner, unix_time_ms: Option<u64>) -> Self {
        let entropy = Sha256::new()
            .chain_update(random_seed.to_le_bytes())
            .chain_update(unix_time_ms.unwrap_or(0).to_le_bytes())
            .finalize()
            .into();
        let mut rng = ChaCha20Rng::from_seed(entropy);
        let id = rng.next_u64();
        info!(
            namespace_id = id,
            unix_time_ms = unix_time_ms,
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
        let ps_exit_map = Mutex::<CriticalSectionRawMutex, PsExitMap>::new(HashMap::default());

        Namespace {
            id,
            spawner,
            creation_time: AtomicU64::new(unix_time_ms.unwrap_or(0)),
            rng: Mutex::<_, _>::new(rng),
            ps_server_set,
            ps_map,
            ps_signal_channel_map,
            ps_ready_map,
            ps_exit_map,
        }
    }

    pub async fn process(
        self: Rc<Self>,
        launcher_set: LauncherSet,
        program_args: Rc<ProgramArgs>,
    ) -> anyhow::Result<Box<dyn Process>> {
        let pid = self.rng.lock().await.next_u64();
        launcher_set
            .launch(ProcessContext {
                pid,
                namespace: self.clone(),
                program_args,
                name: Rc::new(embassy_sync::blocking_mutex::Mutex::new(
                    core::cell::RefCell::new(None),
                )),
            })
            .await
    }

    pub async fn kill(&self, pid: u64, signal: signal::Signal) -> anyhow::Result<()> {
        let channel = self
            .ps_signal_channel_map
            .lock()
            .await
            .get(&pid)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("couldn't find signal channel for process"))?;
        channel.sender().send(signal).await;
        Ok(())
    }

    /// Send SIGTERM to every process in the namespace, yielding between each so
    /// the signalled processes get a chance to run their termination paths.
    pub async fn terminate(&self) {
        let pids: Vec<u64> = self.ps_map.lock().await.keys().copied().collect();
        for pid in pids {
            if let Err(error) = self.kill(pid, signal::Signal::Terminate).await {
                warn!(pid, error = %error, "couldn't terminate process");
            }
            embassy_futures::yield_now().await;
        }
    }
}
