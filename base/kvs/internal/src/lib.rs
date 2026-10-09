//! The store behind the `kvs` program.
//!
//! One [`Kvs`] per namespace, shared by every program that asks [`get_kvs`] for
//! it. The `Kvs` itself is not reference counted; callers hold an `Arc<Kvs>`.
#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::sync::{Arc, Weak};
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};
use dusk_program::anyhow;
use dusk_program::embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::embassy_sync::lazy_lock::LazyLock;
use dusk_program::embassy_sync::mutex::{Mutex, MutexGuard};
use dusk_program::embassy_sync::once_lock::OnceLock;
use dusk_program::embassy_sync::rwlock::RwLock;
use dusk_program::hashbrown::HashMap;
use dusk_program::value::Value;
use nohash_hasher::BuildNoHashHasher;
use store::{Log, State};

mod store;

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Mixed into every key id so kvs ids never collide with a plain fnv1a hash of
/// the same name.
pub const SALT: u64 = 0x9396_8e6e_30a5_93d6;

pub const FLAG_STICKY: u8 = 1;

pub const FLAG_SENSITIVE: u8 = 2;

pub const FLAG_PERSISTENT: u8 = 4;

const DEVICE_ID_KEY: u64 = key_id("dusk.device.id");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sticky {
    pub key: u64,
}

impl core::fmt::Display for Sticky {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "key {:#018x} is sticky: only Dusk sets it",
            self.key
        )
    }
}

impl core::error::Error for Sticky {}

#[cfg(feature = "client")]
pub use linkme;

/// A key name some program registered, paired with the id it hashes to.
#[cfg(feature = "client")]
#[derive(Copy, Clone)]
pub struct KnownKey {
    pub name: &'static str,
    pub id: u64,
}

/// Every key name registered with [`known_key!`], collected at link time.
#[cfg(feature = "client")]
#[linkme::distributed_slice]
pub static KNOWN_KEYS: [KnownKey] = [..];

/// Register a key name, so that a client can show an id under the name it was
/// hashed from.
#[cfg(feature = "client")]
#[macro_export]
macro_rules! known_key {
    ($binding:ident, $name:literal) => {
        #[$crate::linkme::distributed_slice($crate::KNOWN_KEYS)]
        #[linkme(crate = $crate::linkme)]
        static $binding: $crate::KnownKey = $crate::KnownKey {
            name: $name,
            id: $crate::key_id($name),
        };
    };
}

#[cfg(feature = "client")]
#[linkme::distributed_slice]
pub static KNOWN_KEY_LISTS: [&'static [&'static str]] = [..];

#[cfg(feature = "client")]
#[macro_export]
macro_rules! known_keys {
    ($binding:ident, $names:expr) => {
        #[$crate::linkme::distributed_slice($crate::KNOWN_KEY_LISTS)]
        #[linkme(crate = $crate::linkme)]
        static $binding: &'static [&'static str] = $names;
    };
}

/// The name `id` was hashed from, if a program registered it.
#[cfg(feature = "client")]
#[must_use]
pub fn known_key_name(id: u64) -> Option<&'static str> {
    KNOWN_KEYS
        .iter()
        .find(|key| key.id == id)
        .map(|key| key.name)
        .or_else(|| {
            KNOWN_KEY_LISTS
                .iter()
                .flat_map(|names| names.iter())
                .find(|name| key_id(name) == id)
                .copied()
        })
}

/// The id a key name hashes to: fnv1a, 64-bit, salted with [`SALT`].
///
/// `const`, so a program can name its keys at compile time. Distinct names can
/// hash to the same id.
#[must_use]
pub const fn key_id(name: &str) -> u64 {
    let bytes = name.as_bytes();
    let mut hash = FNV_OFFSET_BASIS ^ SALT;
    let mut index = 0;
    while index < bytes.len() {
        hash = (hash ^ (bytes[index] as u64)).wrapping_mul(FNV_PRIME);
        index += 1;
    }
    hash
}

#[must_use]
pub const fn key_ids<const COUNT: usize>(names: [&str; COUNT]) -> [u64; COUNT] {
    let mut ids = [0; COUNT];
    let mut index = 0;
    while index < COUNT {
        ids[index] = key_id(names[index]);
        index += 1;
    }
    ids
}

type Owned = BlockingMutex<
    CriticalSectionRawMutex,
    RefCell<HashMap<u64, alloc::vec::Vec<&'static [u64]>, BuildNoHashHasher<u64>>>,
>;

static OWNED: LazyLock<Owned> =
    LazyLock::new(|| BlockingMutex::new(RefCell::new(HashMap::default())));

pub fn own_keys(tid: u64, keys: &'static [u64]) {
    OWNED
        .get()
        .lock(|owned| owned.borrow_mut().entry(tid).or_default().push(keys));
}

pub fn disown_keys(tid: u64, keys: &'static [u64]) {
    OWNED.get().lock(|owned| {
        let mut owned = owned.borrow_mut();
        if let Some(lists) = owned.get_mut(&tid) {
            if let Some(index) = lists.iter().position(|list| core::ptr::eq(*list, keys)) {
                lists.swap_remove(index);
            }
            if lists.is_empty() {
                owned.remove(&tid);
            }
        }
    });
}

fn owned(tid: u64, key: u64) -> bool {
    OWNED.get().lock(|owned| {
        owned
            .borrow()
            .get(&tid)
            .is_some_and(|lists| lists.iter().any(|list| list.contains(&key)))
    })
}

type Entry = Arc<Mutex<CriticalSectionRawMutex, (Value, u8)>>;

type Entries = HashMap<u64, Entry, BuildNoHashHasher<u64>>;

/// The map changes only when a key first appears or is deleted, so it sits
/// behind an `RwLock` readers share; a key's value changes often, so each entry
/// carries its own `Mutex` and overwriting one takes only the read lock.
pub struct Kvs {
    entries: RwLock<CriticalSectionRawMutex, Entries>,
    tid: u64,
    persistent: OnceLock<Persistent>,
}

struct Persistent {
    tid: u64,
    registration: Registration,
    nonce_seed: u64,
    opened: AtomicBool,
    state: Mutex<CriticalSectionRawMutex, State>,
}

impl Default for Kvs {
    fn default() -> Self {
        Self::new()
    }
}

impl Kvs {
    #[must_use]
    pub fn new() -> Self {
        Kvs {
            entries: RwLock::new(HashMap::default()),
            tid: dusk_core::driver::tid(),
            persistent: OnceLock::new(),
        }
    }

    /// The value stored under `key`, or `None` if the key is absent.
    pub async fn get(&self, key: u64) -> Option<Value> {
        self.get_with_flags(key).await.map(|(value, _)| value)
    }

    pub async fn get_with_flags(&self, key: u64) -> Option<(Value, u8)> {
        self.opened().await;
        self.entry(key).await
    }

    /// Store `value` under `key`, replacing whatever was there.
    pub async fn set(&self, key: u64, value: Value, flags: u8) -> anyhow::Result<()> {
        self.write(key, value, flags, false).await
    }

    pub async fn set_unless_sticky(&self, key: u64, value: Value, flags: u8) -> anyhow::Result<()> {
        self.write(key, value, flags, true).await
    }

    /// Remove `key`, reporting whether it was there.
    pub async fn delete(&self, key: u64) -> anyhow::Result<bool> {
        self.remove(key, false).await
    }

    pub async fn delete_unless_sticky(&self, key: u64) -> anyhow::Result<bool> {
        self.remove(key, true).await
    }

    pub async fn keeps_persistent_keys(&self) -> bool {
        let Some(persistent) = self.persistent.try_get() else {
            return false;
        };
        matches!(*self.store(persistent).await, State::Open(_))
    }

    /// Whether `key` is present.
    pub async fn exists(&self, key: u64) -> bool {
        self.opened().await;
        self.entries.read().await.contains_key(&key)
    }

    /// Every key present at one instant, in no particular order.
    pub async fn scan(&self) -> alloc::vec::Vec<(u64, u8)> {
        self.opened().await;
        let entries = self.entries.read().await;
        let mut keys = alloc::vec::Vec::with_capacity(entries.len());
        for (key, entry) in entries.iter() {
            keys.push((*key, self.flags_of(*key, entry.lock().await.1)));
        }
        keys
    }

    async fn write(
        &self,
        key: u64,
        value: Value,
        flags: u8,
        unless_sticky: bool,
    ) -> anyhow::Result<()> {
        if unless_sticky && owned(self.tid, key) {
            return Err(Sticky { key }.into());
        }
        let Some(persistent) = self.persistent.try_get() else {
            anyhow::ensure!(
                flags & FLAG_PERSISTENT == 0,
                "this node keeps no persistent kvs keys: its kvs launcher was built without a file"
            );
            if self.replace(key, value, flags, unless_sticky).await {
                return Ok(());
            }
            return Err(Sticky { key }.into());
        };
        let mut state = self.store(persistent).await;
        if unless_sticky
            && self
                .entry(key)
                .await
                .is_some_and(|(_, current)| current & FLAG_STICKY != 0)
        {
            return Err(Sticky { key }.into());
        }
        let in_file = matches!(&*state, State::Open(log) if log.entries.contains_key(&key));
        if flags & FLAG_PERSISTENT != 0 || in_file {
            let entry = (flags & FLAG_PERSISTENT != 0).then(|| (value.clone(), flags));
            let dropped = entry.is_none();
            state.log()?.write(key, entry).await?;
            if dropped && flags & FLAG_STICKY != 0 {
                tracing::warn!(
                    path = persistent.registration.path.as_str(),
                    key,
                    "Dusk wrote this key after the persistent kvs file opened; kept its value and dropped the file's"
                );
            }
        }
        self.replace(key, value, flags, false).await;
        compact(&mut state).await;
        Ok(())
    }

    async fn replace(&self, key: u64, value: Value, flags: u8, unless_sticky: bool) -> bool {
        {
            let entries = self.entries.read().await;
            if let Some(entry) = entries.get(&key) {
                return replace(&mut *entry.lock().await, key, value, flags, unless_sticky);
            }
        }
        let mut entries = self.entries.write().await;
        // The key may have appeared while the read guard was released.
        if let Some(entry) = entries.get(&key).cloned() {
            return replace(&mut *entry.lock().await, key, value, flags, unless_sticky);
        }
        entries.insert(key, Arc::new(Mutex::new((value, flags))));
        tracing::debug!(key, flags, "kvs set");
        true
    }

    async fn remove(&self, key: u64, unless_sticky: bool) -> anyhow::Result<bool> {
        if unless_sticky && owned(self.tid, key) {
            return Err(Sticky { key }.into());
        }
        let Some(persistent) = self.persistent.try_get() else {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get(&key).cloned() else {
                return Ok(false);
            };
            if unless_sticky && entry.lock().await.1 & FLAG_STICKY != 0 {
                return Err(Sticky { key }.into());
            }
            entries.remove(&key);
            return Ok(true);
        };
        let mut state = self.store(persistent).await;
        let Some((_, current)) = self.entry(key).await else {
            return Ok(false);
        };
        if unless_sticky && current & FLAG_STICKY != 0 {
            return Err(Sticky { key }.into());
        }
        if let State::Open(log) = &mut *state
            && log.entries.contains_key(&key)
        {
            log.write(key, None).await?;
        }
        self.entries.write().await.remove(&key);
        compact(&mut state).await;
        Ok(true)
    }

    fn flags_of(&self, key: u64, flags: u8) -> u8 {
        if owned(self.tid, key) {
            flags | FLAG_STICKY
        } else {
            flags
        }
    }

    async fn entry(&self, key: u64) -> Option<(Value, u8)> {
        let entry = self.entries.read().await.get(&key).cloned()?;
        let (value, flags) = entry.lock().await.clone();
        Some((value, self.flags_of(key, flags)))
    }

    async fn opened(&self) {
        if let Some(persistent) = self.persistent.try_get()
            && !persistent.opened.load(Ordering::Acquire)
        {
            drop(self.store(persistent).await);
        }
    }

    async fn store<'a>(
        &'a self,
        persistent: &'a Persistent,
    ) -> MutexGuard<'a, CriticalSectionRawMutex, State> {
        let mut state = persistent.state.lock().await;
        let registered = REGISTRATIONS.get().lock(|registrations| {
            registrations.borrow().by_thread.get(&persistent.tid) == Some(&persistent.registration)
        });
        if !registered && !matches!(*state, State::Released) {
            tracing::info!(
                path = persistent.registration.path.as_str(),
                "the kvs launcher that named the persistent kvs file is gone; closed the file"
            );
            *state = State::Released;
        }
        if let State::Closed = *state {
            *state = self.open(persistent).await;
            persistent.opened.store(true, Ordering::Release);
        }
        state
    }

    async fn open(&self, persistent: &Persistent) -> State {
        let path = persistent.registration.path.as_str();
        let device_id = match self.entry(DEVICE_ID_KEY).await {
            Some((Value::String(device_id), _)) => Some(device_id),
            _ => None,
        };
        if device_id.is_none() {
            tracing::info!(
                path,
                "the device has no id: the persistent kvs file's key comes from the fleet token alone"
            );
        }
        let opened = match dusk_core::driver::fs_driver() {
            Ok(file_system) => {
                Log::open(
                    &*file_system,
                    path,
                    dusk_core::fleet_token::fleet_token().as_bytes(),
                    device_id.as_deref().map(str::as_bytes),
                    persistent.nonce_seed,
                )
                .await
            }
            Err(error) => Err(error),
        };
        match opened {
            Ok(mut log) => {
                let mut rewritten = alloc::vec::Vec::new();
                for (key, stored) in &log.entries {
                    if self.entries.read().await.contains_key(key) {
                        rewritten.push(*key);
                        continue;
                    }
                    self.replace(*key, stored.value.clone(), stored.flags, false)
                        .await;
                }
                for key in rewritten {
                    tracing::warn!(
                        path,
                        key,
                        "Dusk wrote this key before the persistent kvs file opened; kept its value and dropped the file's"
                    );
                    if let Err(error) = log.write(key, None).await {
                        tracing::warn!(
                            path,
                            key,
                            error = %alloc::format!("{error:#}"),
                            "couldn't drop a key Dusk rewrote from the persistent kvs file; it is dropped again at the next open"
                        );
                    }
                }
                State::Open(alloc::boxed::Box::new(log))
            }
            Err(error) => {
                let reason = alloc::format!("{error:#}");
                tracing::error!(
                    path,
                    error = reason.as_str(),
                    "the persistent kvs file is unavailable"
                );
                State::Unavailable(reason)
            }
        }
    }

    fn bind(&self, namespace_id: u64, tid: u64, registration: Registration) {
        let path = registration.path.clone();
        let persistent = Persistent {
            tid,
            registration,
            nonce_seed: namespace_id,
            opened: AtomicBool::new(false),
            state: Mutex::new(State::Closed),
        };
        if self.persistent.init(persistent).is_ok() {
            tracing::info!(
                namespace_id,
                path = path.as_str(),
                "the namespace keeps its persistent kvs keys in a file"
            );
        } else {
            tracing::warn!(
                namespace_id,
                "the namespace's kvs was bound to a persistent file twice; kept the first"
            );
        }
    }
}

fn replace(
    entry: &mut (Value, u8),
    key: u64,
    value: Value,
    flags: u8,
    unless_sticky: bool,
) -> bool {
    if unless_sticky && entry.1 & FLAG_STICKY != 0 {
        return false;
    }
    *entry = (value, flags);
    tracing::debug!(key, flags, "kvs set");
    true
}

async fn compact(state: &mut State) {
    let State::Open(log) = state else {
        return;
    };
    if !log.needs_compaction() {
        return;
    }
    match dusk_core::driver::fs_driver() {
        Ok(file_system) => log.compact(&*file_system).await,
        Err(error) => tracing::warn!(
            error = %alloc::format!("{error:#}"),
            "couldn't reach the file system to compact the persistent kvs file"
        ),
    }
}

type Registry = BlockingMutex<
    CriticalSectionRawMutex,
    RefCell<HashMap<u64, Weak<Kvs>, BuildNoHashHasher<u64>>>,
>;

static REGISTRY: LazyLock<Registry> =
    LazyLock::new(|| BlockingMutex::new(RefCell::new(HashMap::default())));

/// The store belonging to `namespace_id`, created on first use.
///
/// This is how a program other than `kvs` reaches the store: keying by
/// namespace is what keeps two namespaces from sharing one.
#[must_use]
pub fn get_kvs(namespace_id: u64) -> Arc<Kvs> {
    let kvs = REGISTRY.get().lock(|registry| {
        let mut registry = registry.borrow_mut();
        registry.retain(|_, kvs| kvs.strong_count() > 0);
        if let Some(kvs) = registry.get(&namespace_id).and_then(Weak::upgrade) {
            return kvs;
        }
        let kvs = Arc::new(Kvs::new());
        registry.insert(namespace_id, Arc::downgrade(&kvs));
        kvs
    });
    if kvs.persistent.try_get().is_none() {
        let tid = dusk_core::driver::tid();
        let registration = REGISTRATIONS
            .get()
            .lock(|registrations| registrations.borrow().by_thread.get(&tid).cloned());
        if let Some(registration) = registration {
            kvs.bind(namespace_id, tid, registration);
        }
    }
    kvs
}

#[derive(Clone, PartialEq, Eq)]
struct Registration {
    path: String,
    generation: u64,
}

struct Registrations {
    by_thread: HashMap<u64, Registration, BuildNoHashHasher<u64>>,
    generation: u64,
}

static REGISTRATIONS: LazyLock<BlockingMutex<CriticalSectionRawMutex, RefCell<Registrations>>> =
    LazyLock::new(|| {
        BlockingMutex::new(RefCell::new(Registrations {
            by_thread: HashMap::default(),
            generation: 0,
        }))
    });

pub fn register_persistent(tid: u64, path: &str) -> anyhow::Result<u64> {
    REGISTRATIONS.get().lock(|registrations| {
        let mut registrations = registrations.borrow_mut();
        if let Some(registered) = registrations.by_thread.get(&tid) {
            anyhow::bail!(
                "thread {tid} already keeps its persistent kvs keys in `{}`",
                registered.path
            );
        }
        if registrations
            .by_thread
            .values()
            .any(|registered| registered.path == path)
        {
            anyhow::bail!("another node of this process keeps its persistent kvs keys in `{path}`");
        }
        registrations.generation += 1;
        let generation = registrations.generation;
        registrations.by_thread.insert(
            tid,
            Registration {
                path: String::from(path),
                generation,
            },
        );
        Ok(generation)
    })
}

pub fn unregister_persistent(tid: u64, generation: u64) {
    REGISTRATIONS.get().lock(|registrations| {
        let mut registrations = registrations.borrow_mut();
        if registrations
            .by_thread
            .get(&tid)
            .is_some_and(|registered| registered.generation == generation)
        {
            registrations.by_thread.remove(&tid);
        }
    });
}
