//! The store behind the `kvs` program.
//!
//! One [`Kvs`] per namespace, shared by every program that asks [`get_kvs`] for
//! it. The `Kvs` itself is not reference counted; callers hold an `Arc<Kvs>`.
#![no_std]

extern crate alloc;

use alloc::sync::Arc;
use core::cell::RefCell;
use dusk_program::embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::embassy_sync::lazy_lock::LazyLock;
use dusk_program::embassy_sync::mutex::Mutex;
use dusk_program::embassy_sync::rwlock::RwLock;
use dusk_program::hashbrown::HashMap;
use dusk_program::value::Value;
use nohash_hasher::BuildNoHashHasher;

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Mixed into every key id so kvs ids never collide with a plain fnv1a hash of
/// the same name.
pub const SALT: u64 = 0x9396_8e6e_30a5_93d6;

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

/// The name `id` was hashed from, if a program registered it.
#[cfg(feature = "client")]
#[must_use]
pub fn known_key_name(id: u64) -> Option<&'static str> {
    KNOWN_KEYS
        .iter()
        .find(|key| key.id == id)
        .map(|key| key.name)
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

type Entry = Arc<Mutex<CriticalSectionRawMutex, Value>>;

type Entries = HashMap<u64, Entry, BuildNoHashHasher<u64>>;

/// The map changes only when a key first appears or is deleted, so it sits
/// behind an `RwLock` readers share; a key's value changes often, so each entry
/// carries its own `Mutex` and overwriting one takes only the read lock.
pub struct Kvs {
    entries: RwLock<CriticalSectionRawMutex, Entries>,
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
        }
    }

    /// The value stored under `key`, or `None` if the key is absent.
    pub async fn get(&self, key: u64) -> Option<Value> {
        let entry = self.entries.read().await.get(&key).cloned()?;
        let value = entry.lock().await.clone();
        Some(value)
    }

    /// Store `value` under `key`, replacing whatever was there.
    pub async fn set(&self, key: u64, value: Value) {
        {
            let entries = self.entries.read().await;
            if let Some(entry) = entries.get(&key) {
                *entry.lock().await = value;
                tracing::debug!(key, "kvs set");
                return;
            }
        }
        let mut entries = self.entries.write().await;
        // The key may have appeared while the read guard was released.
        if let Some(entry) = entries.get(&key).cloned() {
            *entry.lock().await = value;
        } else {
            entries.insert(key, Arc::new(Mutex::new(value)));
        }
        tracing::debug!(key, "kvs set");
    }

    /// Remove `key`, reporting whether it was there.
    pub async fn delete(&self, key: u64) -> bool {
        self.entries.write().await.remove(&key).is_some()
    }

    /// Whether `key` is present.
    pub async fn exists(&self, key: u64) -> bool {
        self.entries.read().await.contains_key(&key)
    }
}

type Registry =
    BlockingMutex<CriticalSectionRawMutex, RefCell<HashMap<u64, Arc<Kvs>, BuildNoHashHasher<u64>>>>;

static REGISTRY: LazyLock<Registry> =
    LazyLock::new(|| BlockingMutex::new(RefCell::new(HashMap::default())));

/// The store belonging to `namespace_id`, created on first use.
///
/// This is how a program other than `kvs` reaches the store: keying by
/// namespace is what keeps two namespaces from sharing one.
#[must_use]
pub fn get_kvs(namespace_id: u64) -> Arc<Kvs> {
    REGISTRY.get().lock(|registry| {
        registry
            .borrow_mut()
            .entry(namespace_id)
            .or_insert_with(|| Arc::new(Kvs::new()))
            .clone()
    })
}
