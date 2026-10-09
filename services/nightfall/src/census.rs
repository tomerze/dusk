use crate::directory::{
    CensusChunk, CensusHeader, CensusSession, Directory, census_sessions, unix_microseconds,
};
use crate::events::{message_id, now};
use crate::kafka::{OutgoingRecord, RecordProducer, default_partition};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

pub const SCHEMA: &str = "dusk.census/v1";
pub const MAXIMUM_CHUNK_SESSIONS: usize = 2000;
pub const MAXIMUM_CHUNK_BYTES: usize = 512 * 1024;
pub const MAXIMUM_CHUNKS: u64 = 10_000;
pub const MAXIMUM_HEARTBEAT_SECONDS: u64 = 3600;
const RECORD_OVERHEAD_BYTES: usize = 1024;

pub fn header_key(instance: &str) -> String {
    format!("{instance}/header")
}

pub fn chunk_key(instance: &str, generation: u64, index: u64) -> String {
    format!("{instance}/{generation}/{index}")
}

fn chunk_message(instance: &str, generation: u64, index: u64, sessions: &[Value]) -> Value {
    json!({
        "schema": SCHEMA,
        "id": message_id(),
        "time": now(),
        "record": "chunk",
        "instance": instance,
        "generation": generation,
        "index": index,
        "sessions": sessions,
    })
}

pub fn chunks(
    instance: &str,
    generation: u64,
    sessions: &[CensusSession],
    maximum_bytes: usize,
) -> anyhow::Result<Vec<Vec<u8>>> {
    split(
        instance,
        generation,
        sessions,
        maximum_bytes,
        MAXIMUM_CHUNK_SESSIONS,
    )
}

fn split(
    instance: &str,
    generation: u64,
    sessions: &[CensusSession],
    maximum_bytes: usize,
    maximum_sessions: usize,
) -> anyhow::Result<Vec<Vec<u8>>> {
    let budget = maximum_bytes
        .min(MAXIMUM_CHUNK_BYTES)
        .saturating_sub(RECORD_OVERHEAD_BYTES);
    let mut encoded = Vec::new();
    let mut current: Vec<Value> = Vec::new();
    let mut current_bytes = 0usize;
    for session in sessions {
        let value = serde_json::to_value(session)?;
        let bytes = serde_json::to_vec(&value)?.len() + 1;
        if bytes > budget {
            anyhow::bail!(
                "a census session of {bytes} bytes does not fit a chunk of {budget} bytes"
            );
        }
        if !current.is_empty()
            && (current.len() >= maximum_sessions || current_bytes + bytes > budget)
        {
            let index = encoded.len() as u64;
            encoded.push(serde_json::to_vec(&chunk_message(
                instance, generation, index, &current,
            ))?);
            current.clear();
            current_bytes = 0;
        }
        current_bytes += bytes;
        current.push(value);
    }
    if !current.is_empty() {
        let index = encoded.len() as u64;
        encoded.push(serde_json::to_vec(&chunk_message(
            instance, generation, index, &current,
        ))?);
    }
    for (index, chunk) in encoded.iter().enumerate() {
        if chunk.len() > maximum_bytes {
            anyhow::bail!(
                "census chunk {index} of {} bytes is above the {maximum_bytes} bytes Kafka accepts",
                chunk.len()
            );
        }
    }
    Ok(encoded)
}

pub fn header_message(header: &CensusHeader) -> Value {
    json!({
        "schema": SCHEMA,
        "id": message_id(),
        "time": header.time,
        "record": "header",
        "instance": header.instance,
        "inner_address": header.inner_address,
        "relay_address": header.relay_address,
        "generation": header.generation,
        "snapshot_epoch": header.snapshot_epoch,
        "started_at": header.started_at,
        "full": header.full,
        "chunk_count": header.chunk_count,
        "session_count": header.session_count,
        "heartbeat_seconds": header.heartbeat_seconds,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum CensusRecord {
    Header(CensusHeader),
    Chunk(CensusChunk),
    Tombstone,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HeaderMessage {
    schema: String,
    id: String,
    time: String,
    record: String,
    instance: String,
    inner_address: String,
    relay_address: String,
    generation: u64,
    snapshot_epoch: u64,
    started_at: String,
    full: bool,
    chunk_count: u64,
    session_count: u64,
    heartbeat_seconds: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChunkMessage {
    schema: String,
    id: String,
    time: String,
    record: String,
    instance: String,
    generation: u64,
    index: u64,
    sessions: Vec<CensusSession>,
}

pub fn parse(payload: Option<&[u8]>) -> Result<CensusRecord, String> {
    let Some(payload) = payload else {
        return Ok(CensusRecord::Tombstone);
    };
    let value: Value = serde_json::from_slice(payload).map_err(|error| error.to_string())?;
    let check = |schema: &str, id: &str, time: &str, instance: &str| -> Result<(), String> {
        if schema != SCHEMA {
            return Err(format!("schema {schema:?}"));
        }
        if uuid::Uuid::parse_str(id).is_err() {
            return Err(format!("id {id:?}"));
        }
        if crate::events::parse_time_milliseconds(time).is_none() {
            return Err(format!("time {time:?}"));
        }
        if instance.is_empty() || instance.contains('/') {
            return Err(format!("instance {instance:?}"));
        }
        Ok(())
    };
    match value.get("record").and_then(Value::as_str) {
        Some("header") => {
            let message: HeaderMessage =
                serde_json::from_value(value).map_err(|error| error.to_string())?;
            check(
                &message.schema,
                &message.id,
                &message.time,
                &message.instance,
            )?;
            if message.record != "header"
                || message.heartbeat_seconds == 0
                || message.heartbeat_seconds > MAXIMUM_HEARTBEAT_SECONDS
                || message.chunk_count > MAXIMUM_CHUNKS
            {
                return Err("an invalid header".to_string());
            }
            Ok(CensusRecord::Header(CensusHeader {
                instance: message.instance,
                inner_address: message.inner_address,
                relay_address: message.relay_address,
                generation: message.generation,
                snapshot_epoch: message.snapshot_epoch,
                started_at: message.started_at,
                full: message.full,
                chunk_count: message.chunk_count,
                session_count: message.session_count,
                heartbeat_seconds: message.heartbeat_seconds,
                time: message.time,
            }))
        }
        Some("chunk") => {
            let message: ChunkMessage =
                serde_json::from_value(value).map_err(|error| error.to_string())?;
            check(
                &message.schema,
                &message.id,
                &message.time,
                &message.instance,
            )?;
            if message.record != "chunk"
                || message.sessions.len() > MAXIMUM_CHUNK_SESSIONS
                || message.index >= MAXIMUM_CHUNKS
            {
                return Err("an invalid chunk".to_string());
            }
            Ok(CensusRecord::Chunk(CensusChunk {
                instance: message.instance,
                generation: message.generation,
                index: message.index,
                sessions: message.sessions,
            }))
        }
        other => Err(format!("record {other:?}")),
    }
}

pub struct CensusSettings {
    pub instance: String,
    pub topic: String,
    pub inner_address: String,
    pub relay_address: String,
    pub heartbeat_seconds: u64,
    pub started_at: String,
}

pub struct CensusProducer {
    settings: CensusSettings,
    producer: Arc<dyn RecordProducer>,
    directory: Arc<Mutex<Directory>>,
    partition: Option<i32>,
    generation: u64,
    chunk_count: u64,
    session_count: u64,
    snapshot_epoch: u64,
    published: bool,
    adopted: bool,
    superseded: Vec<(u64, u64)>,
}

impl CensusProducer {
    pub fn new(
        settings: CensusSettings,
        producer: Arc<dyn RecordProducer>,
        directory: Arc<Mutex<Directory>>,
    ) -> CensusProducer {
        CensusProducer {
            settings,
            producer,
            directory,
            partition: None,
            generation: 0,
            chunk_count: 0,
            session_count: 0,
            snapshot_epoch: 0,
            published: false,
            adopted: false,
            superseded: Vec::new(),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn partition(&mut self) -> Result<i32, String> {
        if let Some(partition) = self.partition {
            return Ok(partition);
        }
        let count = self.producer.partition_count(&self.settings.topic)?;
        let partition = default_partition(&self.settings.instance, count);
        self.partition = Some(partition);
        Ok(partition)
    }

    async fn send(&mut self, key: String, payload: Option<Vec<u8>>) -> Result<(), String> {
        let partition = self.partition()?;
        self.producer
            .send(OutgoingRecord {
                topic: self.settings.topic.clone(),
                partition: Some(partition),
                key: Some(key),
                payload,
            })
            .await
    }

    fn header(&self, full: bool) -> CensusHeader {
        CensusHeader {
            instance: self.settings.instance.clone(),
            inner_address: self.settings.inner_address.clone(),
            relay_address: self.settings.relay_address.clone(),
            generation: self.generation,
            snapshot_epoch: self.snapshot_epoch,
            started_at: self.settings.started_at.clone(),
            full,
            chunk_count: self.chunk_count,
            session_count: self.session_count,
            heartbeat_seconds: self.settings.heartbeat_seconds,
            time: now(),
        }
    }

    async fn publish(
        &mut self,
        sessions: Vec<CensusSession>,
        snapshot_epoch: u64,
    ) -> anyhow::Result<()> {
        let own = self
            .directory
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .own_generation();
        let generation = unix_microseconds()
            .max(self.generation.saturating_add(1))
            .max(own.saturating_add(1));
        let maximum_bytes = self.producer.max_message_bytes();
        let encoded = chunks(
            &self.settings.instance,
            generation,
            &sessions,
            maximum_bytes,
        )?;
        if !self.adopted {
            let mut directory = self
                .directory
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            self.superseded.extend(directory.take_own_chunks());
            self.superseded.extend(directory.own_previous());
            self.adopted = true;
        }
        self.superseded.push((generation, 0));
        for (index, payload) in encoded.iter().enumerate() {
            if let Some(sent) = self.superseded.last_mut() {
                sent.1 = index as u64 + 1;
            }
            self.send(
                chunk_key(&self.settings.instance, generation, index as u64),
                Some(payload.clone()),
            )
            .await
            .map_err(|error| anyhow::anyhow!("census chunk {index}: {error}"))?;
        }
        let header = CensusHeader {
            generation,
            snapshot_epoch,
            chunk_count: encoded.len() as u64,
            session_count: sessions.len() as u64,
            ..self.header(true)
        };
        self.send(
            header_key(&self.settings.instance),
            Some(serde_json::to_vec(&header_message(&header))?),
        )
        .await
        .map_err(|error| anyhow::anyhow!("census header: {error}"))?;
        if self.published {
            self.superseded.push((self.generation, self.chunk_count));
        }
        self.superseded
            .retain(|(superseded, _)| *superseded != generation);
        self.generation = generation;
        self.chunk_count = header.chunk_count;
        self.session_count = header.session_count;
        self.snapshot_epoch = snapshot_epoch;
        self.published = true;
        for (superseded, count) in std::mem::take(&mut self.superseded) {
            let mut kept = false;
            for index in 0..count {
                if let Err(error) = self
                    .send(chunk_key(&self.settings.instance, superseded, index), None)
                    .await
                {
                    tracing::warn!(
                        generation = superseded,
                        index,
                        %error,
                        "a census chunk of a superseded generation was not tombstoned; the next census retries"
                    );
                    kept = true;
                }
            }
            if kept {
                self.superseded.push((superseded, count));
            }
        }
        tracing::info!(
            instance = %self.settings.instance,
            generation,
            chunks = self.chunk_count,
            sessions = self.session_count,
            "census published"
        );
        Ok(())
    }

    pub async fn publish_full(&mut self) -> anyhow::Result<()> {
        let (entries, snapshot_epoch) = self
            .directory
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .census_entries();
        self.publish(census_sessions(&entries), snapshot_epoch)
            .await
    }

    pub async fn publish_final(&mut self) -> anyhow::Result<()> {
        let snapshot_epoch = self
            .directory
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last_epoch();
        self.publish(Vec::new(), snapshot_epoch).await
    }

    pub async fn heartbeat(&mut self) -> anyhow::Result<()> {
        if !self.published {
            return self.publish_full().await;
        }
        let header = serde_json::to_vec(&header_message(&self.header(false)))?;
        self.send(header_key(&self.settings.instance), Some(header))
            .await
            .map_err(|error| anyhow::anyhow!("census heartbeat: {error}"))
    }
}

pub async fn run(
    mut producer: CensusProducer,
    interval: Duration,
    heartbeat: Duration,
    stop: tokio_util::sync::CancellationToken,
) -> CensusProducer {
    let mut next_full = tokio::time::Instant::now();
    let mut failures = 0u32;
    loop {
        let now = tokio::time::Instant::now();
        let outcome = if now >= next_full {
            let outcome = producer.publish_full().await;
            if outcome.is_ok() {
                next_full = now + interval;
            }
            outcome
        } else {
            producer.heartbeat().await
        };
        let wait = match outcome {
            Ok(()) => {
                failures = 0;
                heartbeat
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                metrics::counter!("nightfall_census_failures_total").increment(1);
                tracing::error!(%error, failures, "publishing the census failed");
                crate::backoff::full_jitter(failures, Duration::from_secs(1), heartbeat)
            }
        };
        tokio::select! {
            () = stop.cancelled() => return producer,
            () = tokio::time::sleep(wait) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory::{LocalEntry, NodeIdentity};
    use crate::events::tests::{assert_valid, validator};
    use crate::kafka::{Broker, MemoryBroker, Start};
    use std::sync::atomic::AtomicU64;

    fn session(index: u64, tenant_bytes: usize) -> CensusSession {
        CensusSession {
            device_id: format!("{index:032x}"),
            installation_id: format!("{:032x}", index + 1),
            namespace_id: format!("{index:016x}"),
            epoch: 1_791_000_000_000_000 + index,
            connected_at: "2026-10-08T10:00:00.000000000Z".to_string(),
            last_seen: "2026-10-08T10:00:30.000000000Z".to_string(),
            tenant: (tenant_bytes > 0).then(|| "t".repeat(tenant_bytes.min(63))),
        }
    }

    fn sessions_of(encoded: &[Vec<u8>]) -> Vec<Vec<CensusSession>> {
        encoded
            .iter()
            .map(|payload| match parse(Some(payload)).unwrap() {
                CensusRecord::Chunk(chunk) => chunk.sessions,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn splits_at_the_session_limit() {
        let sessions: Vec<_> = (0..4500).map(|index| session(index, 0)).collect();
        let encoded = split("nightfall-0", 7, &sessions, 1_000_000, 2000).unwrap();
        let by_count = sessions_of(&encoded);
        assert!(by_count.iter().all(|chunk| chunk.len() <= 2000));
        let small = split("nightfall-0", 7, &sessions[..20], 1_000_000, 7).unwrap();
        assert_eq!(
            sessions_of(&small).iter().map(Vec::len).collect::<Vec<_>>(),
            vec![7, 7, 6]
        );
        let validator = validator("dusk.census");
        for payload in &encoded {
            assert_valid(&validator, &serde_json::from_slice(payload).unwrap());
        }
        assert_eq!(by_count.concat(), sessions);
    }

    #[test]
    fn splits_by_bytes_below_the_record_limit() {
        let sessions: Vec<_> = (0..1500).map(|index| session(index, 63)).collect();
        let encoded = chunks("nightfall-0", 7, &sessions, 100_000).unwrap();
        assert!(encoded.len() > 3);
        for payload in &encoded {
            assert!(payload.len() <= 100_000, "{}", payload.len());
        }
        let encoded = chunks("nightfall-0", 7, &sessions, 4_000_000).unwrap();
        for payload in &encoded {
            assert!(payload.len() <= MAXIMUM_CHUNK_BYTES);
        }
        assert_eq!(sessions_of(&encoded).concat(), sessions);
        assert!(chunks("nightfall-0", 7, &sessions, 1100).is_err());
        assert!(chunks("nightfall-0", 7, &[], 1500).unwrap().is_empty());
    }

    #[test]
    fn reads_back_headers_chunks_and_tombstones() {
        let header = CensusHeader {
            instance: "nightfall-0".to_string(),
            inner_address: "nightfall-0.nightfall-inner.dusk.svc:8444".to_string(),
            relay_address: "nightfall-0.nightfall-inner.dusk.svc:8445".to_string(),
            generation: 3,
            snapshot_epoch: 9,
            started_at: now(),
            full: true,
            chunk_count: 1,
            session_count: 1,
            heartbeat_seconds: 15,
            time: now(),
        };
        let message = header_message(&header);
        assert_valid(&validator("dusk.census"), &message);
        assert_eq!(
            parse(Some(message.to_string().as_bytes())).unwrap(),
            CensusRecord::Header(header)
        );
        assert_eq!(parse(None).unwrap(), CensusRecord::Tombstone);
        assert!(parse(Some(b"{\"record\":\"other\"}")).is_err());
        let mut broken = message.clone();
        broken["heartbeat_seconds"] = json!(0);
        assert!(parse(Some(broken.to_string().as_bytes())).is_err());
        assert_eq!(header_key("nightfall-0"), "nightfall-0/header");
        assert_eq!(chunk_key("nightfall-0", 3, 1), "nightfall-0/3/1");
    }

    fn producer_with(
        broker: &Arc<MemoryBroker>,
        sessions: u64,
    ) -> (CensusProducer, Arc<Mutex<Directory>>) {
        let mut directory = Directory::new("nightfall-0");
        for index in 0..sessions {
            let identity = NodeIdentity {
                device: u128::from(index),
                installation: u128::from(index) + 1,
            };
            let issued = directory.issue_epoch(index, identity, 1_000 + index);
            directory.register_local(
                index,
                LocalEntry {
                    shard: 0,
                    identity,
                    epoch: issued.epoch,
                    connected_at: now(),
                    last_seen_ms: Arc::new(AtomicU64::new(1_791_000_000_000)),
                    tenant: None,
                    remote_address: "192.0.2.1:40000".parse().unwrap(),
                    cert_fingerprint: "00".repeat(32),
                    cert_not_after: now(),
                    quarantined: false,
                    epoch_without_history: true,
                },
            );
        }
        let directory = Arc::new(Mutex::new(directory));
        let producer = CensusProducer::new(
            CensusSettings {
                instance: "nightfall-0".to_string(),
                topic: "dusk.census".to_string(),
                inner_address: "nightfall-0.inner:8444".to_string(),
                relay_address: "nightfall-0.inner:8445".to_string(),
                heartbeat_seconds: 15,
                started_at: now(),
            },
            broker.producer("census").unwrap(),
            directory.clone(),
        );
        (producer, directory)
    }

    #[tokio::test]
    async fn publishes_chunks_then_a_header_then_tombstones_the_previous_generation() {
        let broker = MemoryBroker::new();
        broker.create_topic("dusk.census", 6, "compact");
        let (mut producer, _) = producer_with(&broker, 2500);
        producer.publish_full().await.unwrap();
        let first = producer.generation();
        producer.heartbeat().await.unwrap();
        producer.publish_full().await.unwrap();
        let second = producer.generation();
        assert!(second > first);
        let records = broker.records("dusk.census");
        let partition = default_partition("nightfall-0", 6);
        assert!(records.iter().all(|record| record.partition == partition));
        let keys: Vec<(String, bool)> = records
            .iter()
            .map(|record| {
                (
                    String::from_utf8(record.key.clone().unwrap()).unwrap(),
                    record.payload.is_some(),
                )
            })
            .collect();
        let expected = vec![
            (chunk_key("nightfall-0", first, 0), true),
            (chunk_key("nightfall-0", first, 1), true),
            (header_key("nightfall-0"), true),
            (header_key("nightfall-0"), true),
            (chunk_key("nightfall-0", second, 0), true),
            (chunk_key("nightfall-0", second, 1), true),
            (header_key("nightfall-0"), true),
            (chunk_key("nightfall-0", first, 0), false),
            (chunk_key("nightfall-0", first, 1), false),
        ];
        assert_eq!(keys, expected);
        let validator = validator("dusk.census");
        let mut heartbeat_seen = false;
        for record in &records {
            let Some(payload) = &record.payload else {
                continue;
            };
            let value: Value = serde_json::from_slice(payload).unwrap();
            assert_valid(&validator, &value);
            if value["record"] == "header" && value["full"] == false {
                heartbeat_seen = true;
                assert_eq!(value["generation"], first);
                assert_eq!(value["chunk_count"], 2);
            }
        }
        assert!(heartbeat_seen);
    }

    struct Failing {
        inner: Arc<dyn RecordProducer>,
        failing: Mutex<Option<String>>,
    }

    impl RecordProducer for Failing {
        fn send(&self, record: OutgoingRecord) -> crate::kafka::Delivery {
            let failing = self.failing.lock().unwrap().clone();
            if failing.is_some_and(|suffix| record.key.as_deref().unwrap_or("").ends_with(&suffix))
            {
                return Box::pin(async { Err("refused by the test".to_string()) });
            }
            self.inner.send(record)
        }

        fn partition_count(&self, topic: &str) -> Result<u32, String> {
            self.inner.partition_count(topic)
        }

        fn max_message_bytes(&self) -> usize {
            self.inner.max_message_bytes()
        }

        fn flush(&self, timeout: Duration) {
            self.inner.flush(timeout);
        }
    }

    fn live_chunk_keys(broker: &MemoryBroker) -> Vec<String> {
        let mut latest = std::collections::BTreeMap::new();
        for record in broker.records("dusk.census") {
            let key = String::from_utf8(record.key.unwrap()).unwrap();
            latest.insert(key, record.payload.is_some());
        }
        latest
            .into_iter()
            .filter(|(key, live)| *live && !key.ends_with("/header"))
            .map(|(key, _)| key)
            .collect()
    }

    #[tokio::test]
    async fn a_census_that_fails_at_a_chunk_or_its_header_leaves_no_chunk_behind() {
        let broker = MemoryBroker::new();
        broker.create_topic("dusk.census", 1, "compact");
        let (producer, directory) = producer_with(&broker, 2500);
        let failing = Arc::new(Failing {
            inner: broker.producer("census").unwrap(),
            failing: Mutex::new(None),
        });
        let mut producer = CensusProducer::new(producer.settings, failing.clone(), directory);
        producer.publish_full().await.unwrap();
        let first = producer.generation();
        *failing.failing.lock().unwrap() = Some("/1".to_string());
        assert!(producer.publish_full().await.is_err());
        assert_eq!(producer.generation(), first);
        *failing.failing.lock().unwrap() = Some("/header".to_string());
        assert!(producer.publish_full().await.is_err());
        assert_eq!(producer.generation(), first);
        assert_eq!(live_chunk_keys(&broker).len(), 5);
        *failing.failing.lock().unwrap() = None;
        producer.publish_full().await.unwrap();
        let last = producer.generation();
        assert_eq!(
            live_chunk_keys(&broker),
            vec![
                chunk_key("nightfall-0", last, 0),
                chunk_key("nightfall-0", last, 1)
            ]
        );
    }

    #[tokio::test]
    async fn a_restarted_instance_tombstones_the_chunks_its_last_run_left() {
        let broker = MemoryBroker::new();
        broker.create_topic("dusk.census", 1, "compact");
        let (mut producer, directory) = producer_with(&broker, 1);
        {
            let mut directory = directory.lock().unwrap();
            for (generation, index) in [(10, 0), (10, 1), (20, 0), (30, 0)] {
                directory.apply_census_chunk(CensusChunk {
                    instance: "nightfall-0".to_string(),
                    generation,
                    index,
                    sessions: Vec::new(),
                });
            }
            directory.apply_census_header(
                CensusHeader {
                    instance: "nightfall-0".to_string(),
                    inner_address: "nightfall-0.inner:8444".to_string(),
                    relay_address: "nightfall-0.inner:8445".to_string(),
                    generation: 20,
                    snapshot_epoch: 1,
                    started_at: now(),
                    full: true,
                    chunk_count: 1,
                    session_count: 0,
                    heartbeat_seconds: 15,
                    time: now(),
                },
                0,
            );
        }
        producer.publish_full().await.unwrap();
        let tombstoned: std::collections::BTreeSet<String> = broker
            .records("dusk.census")
            .into_iter()
            .filter(|record| record.payload.is_none())
            .map(|record| String::from_utf8(record.key.unwrap()).unwrap())
            .collect();
        assert_eq!(
            tombstoned,
            [
                chunk_key("nightfall-0", 10, 0),
                chunk_key("nightfall-0", 10, 1),
                chunk_key("nightfall-0", 20, 0),
                chunk_key("nightfall-0", 30, 0),
            ]
            .into_iter()
            .collect()
        );
    }

    #[tokio::test]
    async fn a_consumer_rebuilds_the_table_and_a_final_census_empties_it() {
        let broker = MemoryBroker::new();
        broker.create_topic("dusk.census", 1, "compact");
        let (mut producer, _) = producer_with(&broker, 2100);
        producer.publish_full().await.unwrap();
        let mut reader = Directory::new("nightfall-1");
        let mut consumer = broker
            .consumer("census", "dusk.census", Start::Beginning)
            .unwrap();
        let mut apply = |reader: &mut Directory| {
            while let Some(record) = consumer.poll(Duration::from_millis(1)).unwrap() {
                match parse(record.payload.as_deref()).unwrap() {
                    CensusRecord::Header(header) => {
                        reader.apply_census_header(header, crate::kafka::unix_milliseconds());
                    }
                    CensusRecord::Chunk(chunk) => reader.apply_census_chunk(chunk),
                    CensusRecord::Tombstone => {}
                }
            }
        };
        apply(&mut reader);
        assert_eq!(reader.remote_count(), 2100);
        producer.publish_final().await.unwrap();
        apply(&mut reader);
        assert_eq!(reader.remote_count(), 0);
    }
}
