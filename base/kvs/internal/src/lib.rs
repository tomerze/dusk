//! The store behind the `kvs` program.
//!
//! One [`Kvs`] per namespace, shared by every program that asks [`get_kvs`] for
//! it. The `Kvs` itself is not reference counted; callers hold an `Arc<Kvs>`.
#![no_std]

extern crate alloc;

use alloc::sync::{Arc, Weak};
use core::cell::RefCell;
use dusk_program::embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::embassy_sync::lazy_lock::LazyLock;
use dusk_program::embassy_sync::mutex::Mutex;
use dusk_program::embassy_sync::rwlock::RwLock;
use dusk_program::hashbrown::HashMap;
use dusk_program::value::Value;
use nohash_hasher::BuildNoHashHasher;

mod store;

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Mixed into every key id so kvs ids never collide with a plain fnv1a hash of
/// the same name.
pub const SALT: u64 = 0x9396_8e6e_30a5_93d6;

pub const FLAG_STICKY: u8 = 1;

pub const FLAG_SENSITIVE: u8 = 2;

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
        }
    }

    /// The value stored under `key`, or `None` if the key is absent.
    pub async fn get(&self, key: u64) -> Option<Value> {
        self.get_with_flags(key).await.map(|(value, _)| value)
    }

    pub async fn get_with_flags(&self, key: u64) -> Option<(Value, u8)> {
        let entry = self.entries.read().await.get(&key).cloned()?;
        let (value, flags) = entry.lock().await.clone();
        Some((value, self.flags_of(key, flags)))
    }

    /// Store `value` under `key`, replacing whatever was there.
    pub async fn set(&self, key: u64, value: Value, flags: u8) {
        self.write(key, value, flags, false).await;
    }

    pub async fn set_unless_sticky(&self, key: u64, value: Value, flags: u8) -> Result<(), Sticky> {
        if self.write(key, value, flags, true).await {
            Ok(())
        } else {
            Err(Sticky { key })
        }
    }

    async fn write(&self, key: u64, value: Value, flags: u8, unless_sticky: bool) -> bool {
        if unless_sticky && owned(self.tid, key) {
            return false;
        }
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

    /// Remove `key`, reporting whether it was there.
    pub async fn delete(&self, key: u64) -> bool {
        self.entries.write().await.remove(&key).is_some()
    }

    pub async fn delete_unless_sticky(&self, key: u64) -> Result<bool, Sticky> {
        if owned(self.tid, key) {
            return Err(Sticky { key });
        }
        let mut entries = self.entries.write().await;
        let Some(entry) = entries.get(&key).cloned() else {
            return Ok(false);
        };
        if entry.lock().await.1 & FLAG_STICKY != 0 {
            return Err(Sticky { key });
        }
        entries.remove(&key);
        Ok(true)
    }

    /// Whether `key` is present.
    pub async fn exists(&self, key: u64) -> bool {
        self.entries.read().await.contains_key(&key)
    }

    fn flags_of(&self, key: u64, flags: u8) -> u8 {
        if owned(self.tid, key) {
            flags | FLAG_STICKY
        } else {
            flags
        }
    }

    /// Every key present at one instant, in no particular order.
    pub async fn scan(&self) -> alloc::vec::Vec<(u64, u8)> {
        let entries = self.entries.read().await;
        let mut keys = alloc::vec::Vec::with_capacity(entries.len());
        for (key, entry) in entries.iter() {
            keys.push((*key, self.flags_of(*key, entry.lock().await.1)));
        }
        keys
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
    REGISTRY.get().lock(|registry| {
        let mut registry = registry.borrow_mut();
        registry.retain(|_, kvs| kvs.strong_count() > 0);
        if let Some(kvs) = registry.get(&namespace_id).and_then(Weak::upgrade) {
            return kvs;
        }
        let kvs = Arc::new(Kvs::new());
        registry.insert(namespace_id, Arc::downgrade(&kvs));
        kvs
    })
}
