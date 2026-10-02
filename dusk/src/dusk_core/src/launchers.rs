use core::cell::RefCell;
use dusk_program::anyhow::{Result, anyhow};
use dusk_program::embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::embassy_sync::lazy_lock::LazyLock;
use dusk_program::hashbrown::HashMap;
use dusk_program::launcher_set::LauncherSet;
use nohash_hasher::BuildNoHashHasher;

struct Entry {
    launcher_set: LauncherSet,
    _builder: Box<dyn Fn() -> Result<LauncherSet> + Send + Sync>,
}

type Registry =
    BlockingMutex<CriticalSectionRawMutex, RefCell<HashMap<u64, Entry, BuildNoHashHasher<u64>>>>;

static REGISTRY: LazyLock<Registry> =
    LazyLock::new(|| BlockingMutex::new(RefCell::new(HashMap::default())));

pub fn set_launcher_set(
    namespace_id: u64,
    launcher_set: impl Fn() -> Result<LauncherSet> + Send + Sync + 'static,
) -> Result<()> {
    let entry = Entry {
        launcher_set: launcher_set()?,
        _builder: Box::new(launcher_set),
    };
    let replaced = REGISTRY
        .get()
        .lock(|registry| registry.borrow_mut().insert(namespace_id, entry).is_some());

    if replaced {
        tracing::warn!(namespace_id, "replaced the namespace's launcher set");
    } else {
        tracing::info!(namespace_id, "registered the namespace's launcher set");
    }
    Ok(())
}

pub fn remove_launcher_set(namespace_id: u64) {
    let removed = REGISTRY
        .get()
        .lock(|registry| registry.borrow_mut().remove(&namespace_id).is_some());

    if removed {
        tracing::info!(namespace_id, "removed the namespace's launcher set");
    }
}

pub fn launchers(namespace_id: u64) -> Result<LauncherSet> {
    REGISTRY
        .get()
        .lock(|registry| {
            registry
                .borrow()
                .get(&namespace_id)
                .map(|entry| entry.launcher_set.clone())
        })
        .ok_or_else(|| anyhow!("launcher set not found for namespace {namespace_id}"))
}
