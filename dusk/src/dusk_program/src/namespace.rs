use crate::launcher_set::LauncherSet;
use crate::process::Process;
use crate::process::ProcessContext;
use crate::program_args::ProgramArgs;
use crate::ready::Ready;
use crate::signal;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::AtomicU64;
use dusk_capnp::GIT_REV;
use dusk_capnp::capnp_rpc::CapabilityServerSet;
use dusk_capnp::dusk_capnp::process;
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use embassy_sync::watch::Watch;
use hashbrown::HashMap;
use nohash_hasher::BuildNoHashHasher;
use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};
use sha2::{Digest, Sha256};

use tracing::info;
use tracing::warn;

pub type SignalChannel = Channel<NoopRawMutex, signal::Signal, 8>;

pub type PsCapabilityServerSet = CapabilityServerSet<Box<dyn Process>, process::Client>;
pub type ExitWatch = Rc<Watch<CriticalSectionRawMutex, Option<Result<(), String>>, 16>>;
pub type Suspended = Rc<Watch<CriticalSectionRawMutex, bool, 16>>;

#[derive(Clone)]
pub struct PsEntry {
    pub process: process::Client,
    pub channel: Rc<SignalChannel>,
    pub ready: Ready,
    pub exit: ExitWatch,
    pub suspended: Suspended,
}

pub type PsMap = HashMap<u64, PsEntry, BuildNoHashHasher<u64>>;

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

        Namespace {
            id,
            spawner,
            creation_time: AtomicU64::new(unix_time_ms.unwrap_or(0)),
            rng: Mutex::<_, _>::new(rng),
            ps_server_set,
            ps_map,
        }
    }

    pub async fn process(
        self: Rc<Self>,
        launcher_set: LauncherSet,
        program_args: Rc<ProgramArgs>,
    ) -> anyhow::Result<process::Client> {
        let pid = self.rng.lock().await.next_u64();
        let process = launcher_set
            .launch(ProcessContext {
                pid,
                namespace: self.clone(),
                program_args,
                name: Rc::new(embassy_sync::blocking_mutex::Mutex::new(
                    core::cell::RefCell::new(None),
                )),
            })
            .await?;
        let client = self.register(pid, process).await?;
        info!(pid, "process created");
        Ok(client)
    }

    pub async fn entry(&self, pid: u64) -> Option<PsEntry> {
        self.ps_map.lock().await.get(&pid).cloned()
    }

    pub async fn ready(&self, pid: u64) -> Option<bool> {
        self.entry(pid)
            .await
            .map(|entry| entry.ready.try_get().unwrap_or(false))
    }

    pub async fn suspended(&self, pid: u64) -> Option<bool> {
        self.entry(pid)
            .await
            .map(|entry| entry.suspended.try_get().unwrap_or(false))
    }

    pub async fn exited(&self, pid: u64) -> Option<bool> {
        self.entry(pid)
            .await
            .map(|entry| entry.exit.try_get().flatten().is_some())
    }

    pub async fn kill(&self, pid: u64, signal: signal::Signal) -> anyhow::Result<()> {
        if self.exited(pid).await == Some(true) {
            return Err(anyhow::anyhow!("process has exited"));
        }
        if matches!(signal, signal::Signal::Terminate) && self.suspended(pid).await == Some(true) {
            info!(pid, "killed a suspended process");
            self.exit(pid, Ok(())).await;
            return Ok(());
        }
        let entry = self
            .entry(pid)
            .await
            .ok_or_else(|| anyhow::anyhow!("couldn't find process"))?;
        entry.channel.sender().send(signal).await;
        Ok(())
    }

    async fn register(
        &self,
        pid: u64,
        process: Box<dyn Process>,
    ) -> anyhow::Result<process::Client> {
        let mut ps_map = self.ps_map.lock().await;
        if ps_map.contains_key(&pid) {
            return Err(anyhow::anyhow!(
                "pid {pid} is already in use in namespace {}",
                self.id
            ));
        }
        let client = self.ps_server_set.lock().await.new_client(process);
        ps_map.insert(
            pid,
            PsEntry {
                process: client.clone(),
                channel: Rc::new(SignalChannel::new()),
                ready: Rc::new(Watch::new_with(false)),
                exit: Rc::new(Watch::new_with(None)),
                suspended: Rc::new(Watch::new_with(true)),
            },
        );
        Ok(client)
    }

    pub async fn exit(&self, pid: u64, exit: Result<(), String>) {
        let Some(entry) = self.entry(pid).await else {
            return;
        };
        entry.exit.sender().send(Some(exit));
        entry.suspended.sender().send(false);
    }

    pub async fn unregister(&self, pid: u64) -> Option<PsEntry> {
        self.ps_map.lock().await.remove(&pid)
    }

    /// Send SIGTERM to every process in the namespace, yielding between each so
    /// the signalled processes get a chance to run their termination paths.
    pub async fn terminate(&self) {
        let pids: Vec<u64> = self.ps_map.lock().await.keys().copied().collect();
        for pid in pids {
            if self.exited(pid).await == Some(true) {
                continue;
            }
            if let Err(error) = self.kill(pid, signal::Signal::Terminate).await {
                warn!(pid, error = %error, "couldn't terminate process");
            }
            embassy_futures::yield_now().await;
        }
    }
}
