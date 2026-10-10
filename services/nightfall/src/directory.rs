use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEAD_AFTER_HEARTBEATS: u64 = 3;
pub const PENDING_GENERATIONS: usize = 2;
pub const OWN_CHUNK_GENERATIONS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeIdentity {
    pub device: u128,
    pub installation: u128,
}

pub fn parse_identifier(text: &str) -> Option<u128> {
    (text.len() == 32
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    .then(|| u128::from_str_radix(text, 16).ok())
    .flatten()
}

pub fn parse_namespace(text: &str) -> Option<u64> {
    (text.len() == 16
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    .then(|| u64::from_str_radix(text, 16).ok())
    .flatten()
}

pub fn namespace_hex(namespace_id: u64) -> String {
    format!("{namespace_id:016x}")
}

impl NodeIdentity {
    pub fn parse(device_id: &str, installation_id: &str) -> Option<NodeIdentity> {
        Some(NodeIdentity {
            device: parse_identifier(device_id)?,
            installation: parse_identifier(installation_id)?,
        })
    }

    pub fn device_id(&self) -> String {
        format!("{:032x}", self.device)
    }

    pub fn installation_id(&self) -> String {
        format!("{:032x}", self.installation)
    }
}

pub fn unix_microseconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_micros() as u64)
        .unwrap_or(0)
}

#[derive(Clone, Debug)]
pub struct LocalEntry {
    pub shard: usize,
    pub identity: NodeIdentity,
    pub epoch: u64,
    pub connected_at: String,
    pub last_seen_ms: Arc<AtomicU64>,
    pub tenant: Option<String>,
    pub remote_address: SocketAddr,
    pub cert_fingerprint: String,
    pub cert_not_after: String,
    pub quarantined: bool,
    pub epoch_without_history: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteEntry {
    pub instance: Arc<str>,
    pub identity: NodeIdentity,
    pub epoch: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CensusSession {
    pub device_id: String,
    pub installation_id: String,
    pub namespace_id: String,
    pub epoch: u64,
    pub connected_at: String,
    pub last_seen: String,
    pub tenant: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CensusHeader {
    pub instance: String,
    pub inner_address: String,
    pub relay_address: String,
    pub generation: u64,
    pub snapshot_epoch: u64,
    pub started_at: String,
    pub full: bool,
    pub chunk_count: u64,
    pub session_count: u64,
    pub heartbeat_seconds: u64,
    pub time: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CensusChunk {
    pub instance: String,
    pub generation: u64,
    pub index: u64,
    pub sessions: Vec<CensusSession>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionEvent {
    pub connected: bool,
    pub instance: String,
    pub inner_address: String,
    pub identity: NodeIdentity,
    pub namespace_id: u64,
    pub epoch: u64,
}

#[derive(Debug)]
struct Instance {
    name: Arc<str>,
    inner_address: String,
    relay_address: String,
    generation: Option<u64>,
    snapshot_epoch: u64,
    heartbeat_seconds: u64,
    last_header_ms: i64,
    dead_since_ms: Option<i64>,
    pending: BTreeMap<u64, BTreeMap<u64, Vec<CensusSession>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Local {
        shard: usize,
        identity: NodeIdentity,
        epoch: u64,
    },
    Remote {
        instance: Arc<str>,
        relay_address: String,
    },
    Nowhere,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replaced {
    pub namespace_id: u64,
    pub shard: usize,
    pub epoch: u64,
    pub newer_epoch: u64,
    pub instance: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IssuedEpoch {
    pub epoch: u64,
    pub without_history: bool,
}

pub struct Directory {
    instance: Arc<str>,
    local: HashMap<u64, LocalEntry>,
    remote: HashMap<u64, RemoteEntry>,
    instances: HashMap<Arc<str>, Instance>,
    last_epoch: u64,
    own_generation: u64,
    own_chunk_count: u64,
    own_chunks: Option<BTreeMap<u64, u64>>,
}

impl Directory {
    pub fn new(instance: &str) -> Directory {
        Directory {
            instance: Arc::from(instance),
            local: HashMap::new(),
            remote: HashMap::new(),
            instances: HashMap::new(),
            last_epoch: 0,
            own_generation: 0,
            own_chunk_count: 0,
            own_chunks: Some(BTreeMap::new()),
        }
    }

    pub fn instance(&self) -> &str {
        &self.instance
    }

    pub fn route(&self, namespace_id: u64) -> Route {
        if let Some(local) = self.local.get(&namespace_id) {
            return Route::Local {
                shard: local.shard,
                identity: local.identity,
                epoch: local.epoch,
            };
        }
        match self.remote.get(&namespace_id) {
            Some(remote) => match self.instances.get(&remote.instance) {
                Some(instance) if !instance.relay_address.is_empty() => Route::Remote {
                    instance: remote.instance.clone(),
                    relay_address: instance.relay_address.clone(),
                },
                _ => Route::Nowhere,
            },
            None => Route::Nowhere,
        }
    }

    pub fn binding(&self, namespace_id: u64) -> Option<(NodeIdentity, Arc<str>)> {
        if let Some(local) = self.local.get(&namespace_id) {
            return Some((local.identity, self.instance.clone()));
        }
        self.remote
            .get(&namespace_id)
            .map(|remote| (remote.identity, remote.instance.clone()))
    }

    pub fn issue_epoch(
        &mut self,
        namespace_id: u64,
        identity: NodeIdentity,
        now_microseconds: u64,
    ) -> IssuedEpoch {
        let known = self
            .local
            .get(&namespace_id)
            .filter(|local| local.identity == identity)
            .map(|local| local.epoch)
            .into_iter()
            .chain(
                self.remote
                    .get(&namespace_id)
                    .filter(|remote| remote.identity == identity)
                    .map(|remote| remote.epoch),
            )
            .max();
        let epoch = now_microseconds
            .max(self.last_epoch.saturating_add(1))
            .max(known.map_or(0, |known| known.saturating_add(1)));
        self.last_epoch = epoch;
        IssuedEpoch {
            epoch,
            without_history: known.is_none(),
        }
    }

    pub fn last_epoch(&self) -> u64 {
        self.last_epoch
    }

    pub fn seed_epoch(&mut self, now_microseconds: u64) {
        self.last_epoch = self.last_epoch.max(now_microseconds);
    }

    pub fn register_local(&mut self, namespace_id: u64, entry: LocalEntry) -> Option<LocalEntry> {
        self.remote.remove(&namespace_id);
        self.local.insert(namespace_id, entry)
    }

    pub fn unregister_local(&mut self, namespace_id: u64, epoch: u64) -> bool {
        if self
            .local
            .get(&namespace_id)
            .is_some_and(|entry| entry.epoch == epoch)
        {
            self.local.remove(&namespace_id);
            return true;
        }
        false
    }

    pub fn local(&self, namespace_id: u64) -> Option<&LocalEntry> {
        self.local.get(&namespace_id)
    }

    pub fn local_mut(&mut self, namespace_id: u64) -> Option<&mut LocalEntry> {
        self.local.get_mut(&namespace_id)
    }

    pub fn local_entries(&self) -> impl Iterator<Item = (&u64, &LocalEntry)> {
        self.local.iter()
    }

    pub fn local_count(&self) -> usize {
        self.local.len()
    }

    pub fn remote_count(&self) -> usize {
        self.remote.len()
    }

    pub fn remote(&self, namespace_id: u64) -> Option<&RemoteEntry> {
        self.remote.get(&namespace_id)
    }

    pub fn census_entries(&self) -> (Vec<(u64, LocalEntry)>, u64) {
        let entries = self
            .local
            .iter()
            .map(|(namespace_id, entry)| (*namespace_id, entry.clone()))
            .collect();
        (entries, self.last_epoch)
    }

    pub fn own_generation(&self) -> u64 {
        self.own_generation
    }

    pub fn own_previous(&self) -> Option<(u64, u64)> {
        (self.own_generation > 0).then_some((self.own_generation, self.own_chunk_count))
    }

    pub fn take_own_chunks(&mut self) -> Vec<(u64, u64)> {
        let own_generation = self.own_generation;
        self.own_chunks
            .take()
            .unwrap_or_default()
            .into_iter()
            .filter(|(generation, _)| *generation != own_generation)
            .collect()
    }

    fn intern(&mut self, name: &str) -> Arc<str> {
        if let Some((key, _)) = self.instances.get_key_value(name) {
            return key.clone();
        }
        Arc::from(name)
    }

    fn replaced_by(
        &self,
        namespace_id: u64,
        identity: NodeIdentity,
        epoch: u64,
        instance: &str,
    ) -> Option<Replaced> {
        let local = self.local.get(&namespace_id)?;
        if local.identity != identity {
            tracing::error!(
                namespace_id = namespace_hex(namespace_id),
                local_device_id = %local.identity.device_id(),
                local_installation_id = %local.identity.installation_id(),
                device_id = %identity.device_id(),
                installation_id = %identity.installation_id(),
                instance,
                "another instance bound this namespace to a different identity"
            );
            return None;
        }
        if local.epoch >= epoch {
            return None;
        }
        if local.epoch_without_history {
            tracing::warn!(
                namespace_id = namespace_hex(namespace_id),
                device_id = %identity.device_id(),
                installation_id = %identity.installation_id(),
                epoch = local.epoch,
                newer_epoch = epoch,
                instance,
                "an epoch issued from the clock alone is lower than one seen later"
            );
        }
        Some(Replaced {
            namespace_id,
            shard: local.shard,
            epoch: local.epoch,
            newer_epoch: epoch,
            instance: instance.to_string(),
        })
    }

    pub fn apply_connection(&mut self, event: &ConnectionEvent) -> Option<Replaced> {
        if event.instance == *self.instance {
            return None;
        }
        if event.connected {
            let replaced = self.replaced_by(
                event.namespace_id,
                event.identity,
                event.epoch,
                &event.instance,
            );
            let newer = self
                .remote
                .get(&event.namespace_id)
                .is_none_or(|existing| existing.epoch <= event.epoch);
            if newer && !self.local.contains_key(&event.namespace_id) || replaced.is_some() {
                let instance = self.intern(&event.instance);
                self.instances
                    .entry(instance.clone())
                    .or_insert_with(|| Instance {
                        name: instance.clone(),
                        inner_address: event.inner_address.clone(),
                        relay_address: String::new(),
                        generation: None,
                        snapshot_epoch: 0,
                        heartbeat_seconds: 0,
                        last_header_ms: 0,
                        dead_since_ms: None,
                        pending: BTreeMap::new(),
                    });
                self.remote.insert(
                    event.namespace_id,
                    RemoteEntry {
                        instance,
                        identity: event.identity,
                        epoch: event.epoch,
                    },
                );
            }
            replaced
        } else {
            if self
                .remote
                .get(&event.namespace_id)
                .is_some_and(|existing| {
                    *existing.instance == *event.instance && existing.epoch == event.epoch
                })
            {
                self.remote.remove(&event.namespace_id);
            }
            None
        }
    }

    pub fn apply_census_chunk(&mut self, chunk: CensusChunk) {
        if chunk.instance == *self.instance {
            if let Some(own_chunks) = &mut self.own_chunks {
                let count = own_chunks.entry(chunk.generation).or_default();
                *count = (*count).max(chunk.index.saturating_add(1));
                if own_chunks.len() > OWN_CHUNK_GENERATIONS
                    && let Some((forgotten, _)) = own_chunks.pop_first()
                {
                    tracing::warn!(
                        generation = forgotten,
                        "too many generations of this instance's census chunks; the oldest is not tombstoned"
                    );
                }
            }
            return;
        }
        let name = self.intern(&chunk.instance);
        let instance = self
            .instances
            .entry(name.clone())
            .or_insert_with(|| Instance {
                name,
                inner_address: String::new(),
                relay_address: String::new(),
                generation: None,
                snapshot_epoch: 0,
                heartbeat_seconds: 0,
                last_header_ms: 0,
                dead_since_ms: None,
                pending: BTreeMap::new(),
            });
        if instance
            .generation
            .is_some_and(|current| chunk.generation <= current)
        {
            return;
        }
        instance
            .pending
            .entry(chunk.generation)
            .or_default()
            .insert(chunk.index, chunk.sessions);
        while instance.pending.len() > PENDING_GENERATIONS {
            instance.pending.pop_first();
        }
    }

    pub fn apply_census_header(&mut self, header: CensusHeader, header_ms: i64) -> Vec<Replaced> {
        if header.instance == *self.instance {
            if header.generation >= self.own_generation {
                self.own_generation = header.generation;
                self.own_chunk_count = header.chunk_count;
            }
            self.last_epoch = self.last_epoch.max(header.snapshot_epoch);
            return Vec::new();
        }
        let name = self.intern(&header.instance);
        let instance = self
            .instances
            .entry(name.clone())
            .or_insert_with(|| Instance {
                name: name.clone(),
                inner_address: String::new(),
                relay_address: String::new(),
                generation: None,
                snapshot_epoch: 0,
                heartbeat_seconds: 0,
                last_header_ms: 0,
                dead_since_ms: None,
                pending: BTreeMap::new(),
            });
        instance.inner_address = header.inner_address.clone();
        instance.relay_address = header.relay_address.clone();
        instance.heartbeat_seconds = header.heartbeat_seconds;
        instance.last_header_ms = instance.last_header_ms.max(header_ms);
        instance.dead_since_ms = None;
        if instance
            .generation
            .is_some_and(|current| header.generation <= current)
        {
            return Vec::new();
        }
        let complete = header.chunk_count == 0
            || instance
                .pending
                .get(&header.generation)
                .is_some_and(|chunks| {
                    (0..header.chunk_count).all(|index| chunks.contains_key(&index))
                });
        if !complete {
            return Vec::new();
        }
        let chunks = instance
            .pending
            .remove(&header.generation)
            .unwrap_or_default();
        instance
            .pending
            .retain(|generation, _| *generation > header.generation);
        instance.generation = Some(header.generation);
        instance.snapshot_epoch = header.snapshot_epoch;
        let mut present = std::collections::HashSet::new();
        let mut replaced = Vec::new();
        for session in chunks.into_values().flatten() {
            let (Some(namespace_id), Some(identity)) = (
                parse_namespace(&session.namespace_id),
                NodeIdentity::parse(&session.device_id, &session.installation_id),
            ) else {
                tracing::warn!(
                    instance = %header.instance,
                    namespace_id = %session.namespace_id,
                    "a census session holds an invalid id and is skipped"
                );
                continue;
            };
            present.insert(namespace_id);
            if let Some(replacement) =
                self.replaced_by(namespace_id, identity, session.epoch, &header.instance)
            {
                replaced.push(replacement);
            } else if self.local.contains_key(&namespace_id) {
                continue;
            }
            let newer = self
                .remote
                .get(&namespace_id)
                .is_none_or(|existing| existing.epoch <= session.epoch);
            if newer {
                self.remote.insert(
                    namespace_id,
                    RemoteEntry {
                        instance: name.clone(),
                        identity,
                        epoch: session.epoch,
                    },
                );
            }
        }
        let snapshot_epoch = header.snapshot_epoch;
        self.remote.retain(|namespace_id, entry| {
            *entry.instance != *name
                || present.contains(namespace_id)
                || entry.epoch > snapshot_epoch
        });
        tracing::info!(
            instance = %header.instance,
            generation = header.generation,
            sessions = header.session_count,
            chunks = header.chunk_count,
            "census applied"
        );
        replaced
    }

    pub fn oldest_census_ms(&self) -> Option<i64> {
        self.instances
            .values()
            .filter(|instance| instance.dead_since_ms.is_none())
            .filter_map(|instance| instance.generation)
            .map(|generation| i64::try_from(generation / 1000).unwrap_or(i64::MAX))
            .min()
    }

    pub fn expire_instances(&mut self, now_ms: i64) -> Vec<String> {
        let mut dead = Vec::new();
        let mut forgotten = Vec::new();
        for instance in self.instances.values_mut() {
            if instance.heartbeat_seconds == 0 {
                continue;
            }
            let silence = i64::try_from(
                DEAD_AFTER_HEARTBEATS
                    .saturating_mul(instance.heartbeat_seconds)
                    .saturating_mul(1000),
            )
            .unwrap_or(i64::MAX);
            match instance.dead_since_ms {
                None if now_ms.saturating_sub(instance.last_header_ms) > silence => {
                    instance.dead_since_ms = Some(now_ms);
                    dead.push((instance.name.clone(), instance.snapshot_epoch));
                }
                Some(since) if now_ms.saturating_sub(since) > silence => {
                    forgotten.push(instance.name.clone());
                }
                _ => {}
            }
        }
        for name in forgotten {
            let before = self.remote.len();
            self.remote.retain(|_, entry| *entry.instance != *name);
            tracing::info!(
                instance = %name,
                removed = before - self.remote.len(),
                "a dead instance is forgotten with the sessions it announced after its last census"
            );
            self.instances.remove(&name);
        }
        let mut names = Vec::new();
        for (name, snapshot_epoch) in dead {
            let before = self.remote.len();
            self.remote
                .retain(|_, entry| *entry.instance != *name || entry.epoch > snapshot_epoch);
            tracing::warn!(
                instance = %name,
                removed = before - self.remote.len(),
                "an instance stopped publishing its census and is considered dead"
            );
            names.push(name.to_string());
        }
        names
    }
}

pub fn census_sessions(entries: &[(u64, LocalEntry)]) -> Vec<CensusSession> {
    entries
        .iter()
        .map(|(namespace_id, entry)| CensusSession {
            device_id: entry.identity.device_id(),
            installation_id: entry.identity.installation_id(),
            namespace_id: namespace_hex(*namespace_id),
            epoch: entry.epoch,
            connected_at: entry.connected_at.clone(),
            last_seen: crate::events::format_unix_milliseconds(
                entry.last_seen_ms.load(Ordering::Relaxed) as i64,
            ),
            tenant: entry.tenant.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE: &str = "00112233445566778899aabbccddeeff";
    const INSTALLATION: &str = "ffeeddccbbaa99887766554433221100";
    const OTHER_INSTALLATION: &str = "0123456789abcdef0123456789abcdef";

    fn identity() -> NodeIdentity {
        NodeIdentity::parse(DEVICE, INSTALLATION).unwrap()
    }

    fn local(epoch: u64, shard: usize) -> LocalEntry {
        LocalEntry {
            shard,
            identity: identity(),
            epoch,
            connected_at: "2026-10-08T10:00:00.000000000Z".to_string(),
            last_seen_ms: Arc::new(AtomicU64::new(1_791_000_000_000)),
            tenant: None,
            remote_address: "192.0.2.1:40000".parse().unwrap(),
            cert_fingerprint: "00".repeat(32),
            cert_not_after: "2026-10-15T10:00:00.000000000Z".to_string(),
            quarantined: false,
            epoch_without_history: true,
        }
    }

    fn header(
        instance: &str,
        generation: u64,
        chunk_count: u64,
        snapshot_epoch: u64,
    ) -> CensusHeader {
        CensusHeader {
            instance: instance.to_string(),
            inner_address: format!("{instance}.inner:8444"),
            relay_address: format!("{instance}.inner:8445"),
            generation,
            snapshot_epoch,
            started_at: "2026-10-08T10:00:00.000000000Z".to_string(),
            full: true,
            chunk_count,
            session_count: 0,
            heartbeat_seconds: 15,
            time: "2026-10-08T10:00:00.000000000Z".to_string(),
        }
    }

    fn session(namespace_id: u64, epoch: u64) -> CensusSession {
        CensusSession {
            device_id: DEVICE.to_string(),
            installation_id: INSTALLATION.to_string(),
            namespace_id: namespace_hex(namespace_id),
            epoch,
            connected_at: "2026-10-08T10:00:00.000000000Z".to_string(),
            last_seen: "2026-10-08T10:00:00.000000000Z".to_string(),
            tenant: None,
        }
    }

    fn chunk(
        instance: &str,
        generation: u64,
        index: u64,
        sessions: Vec<CensusSession>,
    ) -> CensusChunk {
        CensusChunk {
            instance: instance.to_string(),
            generation,
            index,
            sessions,
        }
    }

    #[test]
    fn parses_and_prints_ids() {
        assert_eq!(identity().device_id(), DEVICE);
        assert_eq!(identity().installation_id(), INSTALLATION);
        assert!(NodeIdentity::parse("00112233445566778899AABBCCDDEEFF", INSTALLATION).is_none());
        assert!(NodeIdentity::parse("0011", INSTALLATION).is_none());
        assert_eq!(parse_namespace("00000000000000ff"), Some(255));
        assert_eq!(parse_namespace("ff"), None);
        assert_eq!(namespace_hex(255), "00000000000000ff");
    }

    #[test]
    fn issues_epochs_above_the_clock_the_last_issued_and_the_last_known() {
        let mut directory = Directory::new("nightfall-0");
        let first = directory.issue_epoch(7, identity(), 1_000);
        assert_eq!(
            first,
            IssuedEpoch {
                epoch: 1_000,
                without_history: true
            }
        );
        let second = directory.issue_epoch(8, identity(), 900);
        assert_eq!(second.epoch, 1_001);
        directory.apply_connection(&ConnectionEvent {
            connected: true,
            instance: "nightfall-1".to_string(),
            inner_address: "nightfall-1.inner:8444".to_string(),
            identity: identity(),
            namespace_id: 9,
            epoch: 50_000,
        });
        let third = directory.issue_epoch(9, identity(), 2_000);
        assert_eq!(
            third,
            IssuedEpoch {
                epoch: 50_001,
                without_history: false
            }
        );
        let other = NodeIdentity::parse(DEVICE, OTHER_INSTALLATION).unwrap();
        let unrelated = directory.issue_epoch(9, other, 2_000);
        assert_eq!(
            unrelated,
            IssuedEpoch {
                epoch: 50_002,
                without_history: true
            }
        );
        assert_eq!(directory.last_epoch(), 50_002);
    }

    #[test]
    fn routes_to_local_remote_or_nowhere() {
        let mut directory = Directory::new("nightfall-0");
        assert_eq!(directory.route(1), Route::Nowhere);
        directory.register_local(1, local(10, 3));
        assert_eq!(
            directory.route(1),
            Route::Local {
                shard: 3,
                identity: identity(),
                epoch: 10
            }
        );
        directory.apply_census_chunk(chunk("nightfall-1", 5, 0, vec![session(2, 20)]));
        directory.apply_census_header(header("nightfall-1", 5, 1, 20), 1_000);
        assert_eq!(
            directory.route(2),
            Route::Remote {
                instance: Arc::from("nightfall-1"),
                relay_address: "nightfall-1.inner:8445".to_string()
            }
        );
        assert!(!directory.unregister_local(1, 11));
        assert!(directory.unregister_local(1, 10));
        assert_eq!(directory.route(1), Route::Nowhere);
    }

    #[test]
    fn switches_a_generation_only_once_every_chunk_and_its_header_are_read() {
        let mut directory = Directory::new("nightfall-0");
        directory.apply_census_chunk(chunk("nightfall-1", 5, 1, vec![session(2, 20)]));
        directory.apply_census_header(header("nightfall-1", 5, 2, 30), 1_000);
        assert_eq!(directory.remote_count(), 0);
        directory.apply_census_chunk(chunk("nightfall-1", 5, 0, vec![session(3, 21)]));
        assert_eq!(directory.remote_count(), 0);
        directory.apply_census_header(header("nightfall-1", 5, 2, 30), 2_000);
        assert_eq!(directory.remote_count(), 2);
        directory.apply_census_chunk(chunk("nightfall-1", 6, 0, vec![session(3, 21)]));
        directory.apply_census_header(header("nightfall-1", 6, 1, 40), 3_000);
        assert_eq!(directory.remote_count(), 1);
        assert!(directory.remote(2).is_none());
        directory.apply_census_chunk(chunk("nightfall-1", 4, 0, vec![session(9, 1)]));
        directory.apply_census_header(header("nightfall-1", 4, 1, 1), 4_000);
        assert!(directory.remote(9).is_none());
    }

    #[test]
    fn keeps_sessions_connected_after_the_snapshot_and_empties_on_a_zero_chunk_census() {
        let mut directory = Directory::new("nightfall-0");
        directory.apply_connection(&ConnectionEvent {
            connected: true,
            instance: "nightfall-1".to_string(),
            inner_address: "nightfall-1.inner:8444".to_string(),
            identity: identity(),
            namespace_id: 4,
            epoch: 100,
        });
        directory.apply_census_header(header("nightfall-1", 7, 0, 99), 1_000);
        assert!(directory.remote(4).is_some());
        directory.apply_census_header(header("nightfall-1", 8, 0, 100), 2_000);
        assert!(directory.remote(4).is_none());
    }

    #[test]
    fn a_newer_session_elsewhere_replaces_the_local_one() {
        let mut directory = Directory::new("nightfall-0");
        directory.register_local(1, local(10, 2));
        let event = ConnectionEvent {
            connected: true,
            instance: "nightfall-1".to_string(),
            inner_address: "nightfall-1.inner:8444".to_string(),
            identity: identity(),
            namespace_id: 1,
            epoch: 11,
        };
        let replaced = directory.apply_connection(&event).unwrap();
        assert_eq!(replaced.shard, 2);
        assert_eq!(replaced.epoch, 10);
        assert_eq!(replaced.newer_epoch, 11);
        assert!(directory.remote(1).is_some());
        let stale = ConnectionEvent {
            epoch: 9,
            ..event.clone()
        };
        let mut fresh = Directory::new("nightfall-0");
        fresh.register_local(1, local(10, 2));
        assert!(fresh.apply_connection(&stale).is_none());
        let own = ConnectionEvent {
            instance: "nightfall-0".to_string(),
            ..event
        };
        assert!(fresh.apply_connection(&own).is_none());
        fresh.apply_census_chunk(chunk("nightfall-1", 1, 0, vec![session(1, 12)]));
        let replaced = fresh.apply_census_header(header("nightfall-1", 1, 1, 12), 1_000);
        assert_eq!(replaced.len(), 1);
    }

    #[test]
    fn a_disconnect_removes_only_the_matching_remote_entry() {
        let mut directory = Directory::new("nightfall-0");
        let connected = ConnectionEvent {
            connected: true,
            instance: "nightfall-1".to_string(),
            inner_address: "nightfall-1.inner:8444".to_string(),
            identity: identity(),
            namespace_id: 4,
            epoch: 100,
        };
        directory.apply_connection(&connected);
        directory.apply_connection(&ConnectionEvent {
            connected: false,
            epoch: 99,
            ..connected.clone()
        });
        assert!(directory.remote(4).is_some());
        directory.apply_connection(&ConnectionEvent {
            connected: false,
            ..connected
        });
        assert!(directory.remote(4).is_none());
    }

    #[test]
    fn a_dead_instance_loses_only_the_entries_of_its_census() {
        let mut directory = Directory::new("nightfall-0");
        directory.apply_census_chunk(chunk("nightfall-1", 1, 0, vec![session(2, 20)]));
        directory.apply_census_header(header("nightfall-1", 1, 1, 20), 10_000);
        directory.apply_connection(&ConnectionEvent {
            connected: true,
            instance: "nightfall-1".to_string(),
            inner_address: "nightfall-1.inner:8444".to_string(),
            identity: NodeIdentity::parse(DEVICE, OTHER_INSTALLATION).unwrap(),
            namespace_id: 3,
            epoch: 25,
        });
        directory.apply_census_chunk(chunk("nightfall-2", 1, 0, vec![session(5, 30)]));
        directory.apply_census_header(header("nightfall-2", 1, 1, 30), 50_000);
        assert!(directory.expire_instances(50_000).is_empty());
        assert_eq!(
            directory.expire_instances(55_001),
            vec!["nightfall-1".to_string()]
        );
        assert!(directory.remote(2).is_none());
        assert!(directory.remote(3).is_some());
        assert!(directory.remote(5).is_some());
        assert_eq!(directory.oldest_census_ms(), Some(0));
        directory.apply_census_header(
            CensusHeader {
                full: false,
                ..header("nightfall-2", 1, 1, 30)
            },
            95_000,
        );
        assert!(directory.expire_instances(100_002).is_empty());
        assert!(directory.remote(3).is_none());
        assert!(directory.remote(5).is_some());
        assert_eq!(directory.remote_count(), 1);
    }

    #[test]
    fn connections_are_read_from_the_snapshot_of_the_oldest_applied_census() {
        let snapshot_ms = 1_791_000_000_000i64;
        let generation = snapshot_ms as u64 * 1000;
        let mut directory = Directory::new("nightfall-0");
        assert_eq!(directory.oldest_census_ms(), None);
        directory.apply_census_chunk(chunk("nightfall-1", generation, 0, vec![session(2, 20)]));
        directory.apply_census_header(header("nightfall-1", generation, 1, 20), snapshot_ms);
        let heartbeat = CensusHeader {
            full: false,
            ..header("nightfall-1", generation, 1, 20)
        };
        directory.apply_census_header(heartbeat, snapshot_ms + 285_000);
        directory.apply_census_chunk(chunk(
            "nightfall-2",
            generation + 60_000_000,
            0,
            vec![session(3, 30)],
        ));
        directory.apply_census_header(
            header("nightfall-2", generation + 60_000_000, 1, 30),
            snapshot_ms + 290_000,
        );
        assert_eq!(directory.oldest_census_ms(), Some(snapshot_ms));
    }

    #[test]
    fn records_its_own_generation_and_never_its_own_sessions() {
        let mut directory = Directory::new("nightfall-0");
        directory.apply_census_chunk(chunk("nightfall-0", 9, 0, vec![session(2, 20)]));
        directory.apply_census_header(header("nightfall-0", 9, 1, 20), 1_000);
        assert_eq!(directory.own_generation(), 9);
        assert_eq!(directory.remote_count(), 0);
    }

    #[test]
    fn copies_the_local_table_for_the_census() {
        let mut directory = Directory::new("nightfall-0");
        directory.issue_epoch(1, identity(), 10);
        directory.register_local(1, local(10, 0));
        let (entries, snapshot_epoch) = directory.census_entries();
        let sessions = census_sessions(&entries);
        assert_eq!(snapshot_epoch, 10);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].namespace_id, "0000000000000001");
        assert_eq!(sessions[0].last_seen.len(), 30);
    }
}
