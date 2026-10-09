use anyhow::{Context, bail};
use rdkafka::ClientConfig;
use rdkafka::admin::{AdminClient, AdminOptions, ResourceSpecifier};
use rdkafka::client::DefaultClientContext;
use rdkafka::consumer::{BaseConsumer, Consumer};
use rdkafka::error::KafkaError;
use rdkafka::message::Message;
use rdkafka::producer::{FutureProducer, FutureRecord, Producer};
use rdkafka::topic_partition_list::{Offset, TopicPartitionList};
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 1_000_000;
const METADATA_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutgoingRecord {
    pub topic: String,
    pub partition: Option<i32>,
    pub key: Option<String>,
    pub payload: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncomingRecord {
    pub topic: String,
    pub partition: i32,
    pub offset: i64,
    pub key: Option<Vec<u8>>,
    pub payload: Option<Vec<u8>>,
    pub timestamp_ms: Option<i64>,
}

pub type Delivery = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

pub trait RecordProducer: Send + Sync {
    fn send(&self, record: OutgoingRecord) -> Delivery;
    fn partition_count(&self, topic: &str) -> Result<u32, String>;
    fn max_message_bytes(&self) -> usize;
    fn flush(&self, timeout: Duration);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Watermarks {
    pub partition: i32,
    pub low: i64,
    pub high: i64,
}

pub trait RecordConsumer: Send {
    fn poll(&mut self, timeout: Duration) -> Result<Option<IncomingRecord>, String>;
    fn watermarks(&mut self) -> Result<Vec<Watermarks>, String>;
    fn positions(&mut self) -> Result<Vec<(i32, Option<i64>)>, String>;
}

pub fn caught_up(targets: &[Watermarks], positions: &[(i32, Option<i64>)]) -> bool {
    targets.iter().all(|target| {
        target.high <= target.low
            || positions.iter().any(|(partition, position)| {
                *partition == target.partition
                    && position.is_some_and(|position| position >= target.high)
            })
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    Beginning,
    TimeMilliseconds(i64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicDescription {
    pub partitions: u32,
    pub cleanup_policy: String,
}

pub trait Broker: Send + Sync {
    fn producer(&self, role: &str) -> anyhow::Result<Arc<dyn RecordProducer>>;
    fn consumer(
        &self,
        role: &str,
        topic: &str,
        start: Start,
    ) -> anyhow::Result<Box<dyn RecordConsumer>>;
    fn describe(&self, topic: &str) -> anyhow::Result<Option<TopicDescription>>;
}

pub fn unix_milliseconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Clone, Debug)]
struct StoredRecord {
    key: Option<Vec<u8>>,
    payload: Option<Vec<u8>>,
    timestamp_ms: i64,
}

struct MemoryTopic {
    cleanup_policy: String,
    partitions: Vec<Vec<StoredRecord>>,
}

#[derive(Default)]
struct MemoryState {
    topics: HashMap<String, MemoryTopic>,
    failing: HashMap<String, String>,
}

#[derive(Default)]
pub struct MemoryBroker {
    state: Mutex<MemoryState>,
    changed: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn murmur2(bytes: &[u8]) -> i32 {
    const SEED: u32 = 0x9747_b28c;
    const MULTIPLIER: u32 = 0x5bd1_e995;
    const SHIFT: u32 = 24;
    let length = bytes.len() as u32;
    let mut hash = SEED ^ length;
    let mut chunks = bytes.chunks_exact(4);
    for chunk in &mut chunks {
        let mut word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        word = word.wrapping_mul(MULTIPLIER);
        word ^= word >> SHIFT;
        word = word.wrapping_mul(MULTIPLIER);
        hash = hash.wrapping_mul(MULTIPLIER);
        hash ^= word;
    }
    let rest = chunks.remainder();
    if rest.len() >= 3 {
        hash ^= u32::from(rest[2]) << 16;
    }
    if rest.len() >= 2 {
        hash ^= u32::from(rest[1]) << 8;
    }
    if !rest.is_empty() {
        hash ^= u32::from(rest[0]);
        hash = hash.wrapping_mul(MULTIPLIER);
    }
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(MULTIPLIER);
    hash ^= hash >> 15;
    hash as i32
}

pub fn default_partition(key: &str, partitions: u32) -> i32 {
    ((murmur2(key.as_bytes()) & 0x7fff_ffff) as u32 % partitions.max(1)) as i32
}

impl MemoryBroker {
    pub fn new() -> Arc<MemoryBroker> {
        Arc::new(MemoryBroker::default())
    }

    pub fn create_topic(&self, topic: &str, partitions: u32, cleanup_policy: &str) {
        lock(&self.state).topics.insert(
            topic.to_string(),
            MemoryTopic {
                cleanup_policy: cleanup_policy.to_string(),
                partitions: vec![Vec::new(); partitions.max(1) as usize],
            },
        );
    }

    pub fn fail_topic(&self, topic: &str, reason: Option<&str>) {
        let mut state = lock(&self.state);
        match reason {
            Some(reason) => {
                state.failing.insert(topic.to_string(), reason.to_string());
            }
            None => {
                state.failing.remove(topic);
            }
        }
    }

    pub fn records(&self, topic: &str) -> Vec<IncomingRecord> {
        let state = lock(&self.state);
        let Some(stored) = state.topics.get(topic) else {
            return Vec::new();
        };
        stored
            .partitions
            .iter()
            .enumerate()
            .flat_map(|(partition, records)| {
                records
                    .iter()
                    .enumerate()
                    .map(move |(offset, record)| IncomingRecord {
                        topic: topic.to_string(),
                        partition: partition as i32,
                        offset: offset as i64,
                        key: record.key.clone(),
                        payload: record.payload.clone(),
                        timestamp_ms: Some(record.timestamp_ms),
                    })
            })
            .collect()
    }

    pub fn produce(&self, record: OutgoingRecord) -> Result<(), String> {
        let mut state = lock(&self.state);
        if let Some(reason) = state.failing.get(&record.topic) {
            return Err(reason.clone());
        }
        let Some(topic) = state.topics.get_mut(&record.topic) else {
            return Err(format!("unknown topic {}", record.topic));
        };
        let count = topic.partitions.len() as u32;
        let partition = record.partition.unwrap_or_else(|| {
            record
                .key
                .as_deref()
                .map_or(0, |key| default_partition(key, count))
        });
        let Some(records) = topic.partitions.get_mut(partition as usize) else {
            return Err(format!("{} has no partition {partition}", record.topic));
        };
        records.push(StoredRecord {
            key: record.key.map(String::into_bytes),
            payload: record.payload,
            timestamp_ms: unix_milliseconds(),
        });
        drop(state);
        self.changed.notify_all();
        Ok(())
    }
}

struct MemoryProducer {
    broker: Arc<MemoryBroker>,
}

impl RecordProducer for MemoryProducer {
    fn send(&self, record: OutgoingRecord) -> Delivery {
        let outcome = self.broker.produce(record);
        Box::pin(async move { outcome })
    }

    fn partition_count(&self, topic: &str) -> Result<u32, String> {
        lock(&self.broker.state)
            .topics
            .get(topic)
            .map(|stored| stored.partitions.len() as u32)
            .ok_or_else(|| format!("unknown topic {topic}"))
    }

    fn max_message_bytes(&self) -> usize {
        DEFAULT_MAX_MESSAGE_BYTES
    }

    fn flush(&self, _timeout: Duration) {}
}

struct MemoryConsumer {
    broker: Arc<MemoryBroker>,
    topic: String,
    positions: Vec<i64>,
}

impl RecordConsumer for MemoryConsumer {
    fn poll(&mut self, timeout: Duration) -> Result<Option<IncomingRecord>, String> {
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.broker.state);
        loop {
            if let Some(stored) = state.topics.get(&self.topic) {
                for (partition, records) in stored.partitions.iter().enumerate() {
                    let Some(position) = self.positions.get_mut(partition) else {
                        continue;
                    };
                    if let Some(record) = records.get(*position as usize) {
                        let incoming = IncomingRecord {
                            topic: self.topic.clone(),
                            partition: partition as i32,
                            offset: *position,
                            key: record.key.clone(),
                            payload: record.payload.clone(),
                            timestamp_ms: Some(record.timestamp_ms),
                        };
                        *position += 1;
                        return Ok(Some(incoming));
                    }
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            state = self
                .broker
                .changed
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }

    fn watermarks(&mut self) -> Result<Vec<Watermarks>, String> {
        let state = lock(&self.broker.state);
        let stored = state
            .topics
            .get(&self.topic)
            .ok_or_else(|| format!("unknown topic {}", self.topic))?;
        Ok(stored
            .partitions
            .iter()
            .enumerate()
            .map(|(partition, records)| Watermarks {
                partition: partition as i32,
                low: 0,
                high: records.len() as i64,
            })
            .collect())
    }

    fn positions(&mut self) -> Result<Vec<(i32, Option<i64>)>, String> {
        Ok(self
            .positions
            .iter()
            .enumerate()
            .map(|(partition, position)| (partition as i32, Some(*position)))
            .collect())
    }
}

impl Broker for Arc<MemoryBroker> {
    fn producer(&self, _role: &str) -> anyhow::Result<Arc<dyn RecordProducer>> {
        Ok(Arc::new(MemoryProducer {
            broker: self.clone(),
        }))
    }

    fn consumer(
        &self,
        _role: &str,
        topic: &str,
        start: Start,
    ) -> anyhow::Result<Box<dyn RecordConsumer>> {
        let state = lock(&self.state);
        let stored = state
            .topics
            .get(topic)
            .with_context(|| format!("unknown topic {topic}"))?;
        let positions = stored
            .partitions
            .iter()
            .map(|records| match start {
                Start::Beginning => 0,
                Start::TimeMilliseconds(time) => records
                    .iter()
                    .position(|record| record.timestamp_ms >= time)
                    .unwrap_or(records.len())
                    as i64,
            })
            .collect();
        Ok(Box::new(MemoryConsumer {
            broker: self.clone(),
            topic: topic.to_string(),
            positions,
        }))
    }

    fn describe(&self, topic: &str) -> anyhow::Result<Option<TopicDescription>> {
        Ok(lock(&self.state)
            .topics
            .get(topic)
            .map(|stored| TopicDescription {
                partitions: stored.partitions.len() as u32,
                cleanup_policy: stored.cleanup_policy.clone(),
            }))
    }
}

pub struct KafkaBroker {
    brokers: String,
    properties: BTreeMap<String, String>,
    instance: String,
}

impl KafkaBroker {
    pub fn new(
        brokers: &str,
        properties: &BTreeMap<String, String>,
        instance: &str,
    ) -> KafkaBroker {
        KafkaBroker {
            brokers: brokers.to_string(),
            properties: properties.clone(),
            instance: instance.to_string(),
        }
    }

    fn base_config(&self, client_id: &str) -> ClientConfig {
        let mut config = ClientConfig::new();
        for (name, value) in &self.properties {
            config.set(name, value);
        }
        config
            .set("bootstrap.servers", &self.brokers)
            .set("client.id", client_id)
            .set("allow.auto.create.topics", "false");
        config
    }

    pub fn producer_config(&self, role: &str) -> ClientConfig {
        let mut config = self.base_config(&format!("nightfall-{role}-{}", self.instance));
        config
            .set("enable.idempotence", "true")
            .set("acks", "all")
            .set("compression.type", "zstd")
            .set("linger.ms", "5")
            .set("message.timeout.ms", "120000");
        config
    }
}

struct KafkaProducer {
    producer: FutureProducer,
    max_message_bytes: usize,
}

impl RecordProducer for KafkaProducer {
    fn send(&self, record: OutgoingRecord) -> Delivery {
        let mut future_record: FutureRecord<'_, str, [u8]> = FutureRecord::to(&record.topic);
        if let Some(key) = record.key.as_deref() {
            future_record = future_record.key(key);
        }
        if let Some(payload) = record.payload.as_deref() {
            future_record = future_record.payload(payload);
        }
        if let Some(partition) = record.partition {
            future_record = future_record.partition(partition);
        }
        match self.producer.send_result(future_record) {
            Ok(delivery) => Box::pin(async move {
                match delivery.await {
                    Ok(Ok(_)) => Ok(()),
                    Ok(Err((error, _))) => Err(error.to_string()),
                    Err(_) => Err("the producer dropped the delivery".to_string()),
                }
            }),
            Err((error, _)) => {
                let message = error.to_string();
                Box::pin(async move { Err(message) })
            }
        }
    }

    fn partition_count(&self, topic: &str) -> Result<u32, String> {
        let metadata = self
            .producer
            .client()
            .fetch_metadata(Some(topic), METADATA_TIMEOUT)
            .map_err(|error| error.to_string())?;
        metadata
            .topics()
            .iter()
            .find(|described| described.name() == topic && described.error().is_none())
            .map(|described| described.partitions().len() as u32)
            .filter(|count| *count > 0)
            .ok_or_else(|| format!("the broker reports no topic {topic}"))
    }

    fn max_message_bytes(&self) -> usize {
        self.max_message_bytes
    }

    fn flush(&self, timeout: Duration) {
        if let Err(error) = self.producer.flush(timeout) {
            tracing::warn!(%error, "flushing a Kafka producer did not finish");
        }
    }
}

struct KafkaConsumer {
    consumer: BaseConsumer,
    topic: String,
    partitions: Vec<i32>,
}

impl RecordConsumer for KafkaConsumer {
    fn poll(&mut self, timeout: Duration) -> Result<Option<IncomingRecord>, String> {
        match self.consumer.poll(timeout) {
            None => Ok(None),
            Some(Err(KafkaError::PartitionEOF(_))) => Ok(None),
            Some(Err(error)) => Err(error.to_string()),
            Some(Ok(message)) => Ok(Some(IncomingRecord {
                topic: message.topic().to_string(),
                partition: message.partition(),
                offset: message.offset(),
                key: message.key().map(<[u8]>::to_vec),
                payload: message.payload().map(<[u8]>::to_vec),
                timestamp_ms: message.timestamp().to_millis(),
            })),
        }
    }

    fn watermarks(&mut self) -> Result<Vec<Watermarks>, String> {
        self.partitions
            .iter()
            .map(|partition| {
                self.consumer
                    .fetch_watermarks(&self.topic, *partition, METADATA_TIMEOUT)
                    .map(|(low, high)| Watermarks {
                        partition: *partition,
                        low,
                        high,
                    })
                    .map_err(|error| error.to_string())
            })
            .collect()
    }

    fn positions(&mut self) -> Result<Vec<(i32, Option<i64>)>, String> {
        let positions = self
            .consumer
            .position()
            .map_err(|error| error.to_string())?;
        Ok(positions
            .elements()
            .iter()
            .map(|element| {
                let position = match element.offset() {
                    Offset::Offset(offset) => Some(offset),
                    _ => None,
                };
                (element.partition(), position)
            })
            .collect())
    }
}

impl Broker for KafkaBroker {
    fn producer(&self, role: &str) -> anyhow::Result<Arc<dyn RecordProducer>> {
        let config = self.producer_config(role);
        let max_message_bytes = config
            .get("message.max.bytes")
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_MAX_MESSAGE_BYTES);
        let producer: FutureProducer = config
            .create()
            .with_context(|| format!("create the Kafka {role} producer"))?;
        Ok(Arc::new(KafkaProducer {
            producer,
            max_message_bytes,
        }))
    }

    fn consumer(
        &self,
        role: &str,
        topic: &str,
        start: Start,
    ) -> anyhow::Result<Box<dyn RecordConsumer>> {
        let client_id = format!("nightfall-{role}-{}", self.instance);
        let mut config = self.base_config(&client_id);
        config
            .set("group.id", &client_id)
            .set("enable.auto.commit", "false")
            .set("enable.auto.offset.store", "false")
            .set("enable.partition.eof", "false")
            .set("auto.offset.reset", "earliest")
            .set("isolation.level", "read_committed");
        let consumer: BaseConsumer = config
            .create()
            .with_context(|| format!("create the Kafka {role} consumer"))?;
        let metadata = consumer
            .fetch_metadata(Some(topic), METADATA_TIMEOUT)
            .with_context(|| format!("read the metadata of {topic}"))?;
        let Some(described) = metadata
            .topics()
            .iter()
            .find(|described| described.name() == topic && described.error().is_none())
        else {
            bail!("the broker reports no topic {topic}");
        };
        let partitions: Vec<i32> = described
            .partitions()
            .iter()
            .map(|partition| partition.id())
            .collect();
        let mut assignment = TopicPartitionList::new();
        for partition in &partitions {
            let offset = match start {
                Start::Beginning => Offset::Beginning,
                Start::TimeMilliseconds(time) => Offset::Offset(time),
            };
            assignment
                .add_partition_offset(topic, *partition, offset)
                .with_context(|| format!("assign {topic} partition {partition}"))?;
        }
        if let Start::TimeMilliseconds(_) = start {
            assignment = consumer
                .offsets_for_times(assignment, METADATA_TIMEOUT)
                .with_context(|| format!("look up the {topic} offsets by time"))?;
            for element in assignment.clone().elements() {
                if element.offset() == Offset::End || element.offset() == Offset::Invalid {
                    assignment
                        .set_partition_offset(topic, element.partition(), Offset::End)
                        .with_context(|| {
                            format!("assign {topic} partition {}", element.partition())
                        })?;
                }
            }
        }
        consumer
            .assign(&assignment)
            .with_context(|| format!("assign the partitions of {topic}"))?;
        Ok(Box::new(KafkaConsumer {
            consumer,
            topic: topic.to_string(),
            partitions,
        }))
    }

    fn describe(&self, topic: &str) -> anyhow::Result<Option<TopicDescription>> {
        let client_id = format!("nightfall-admin-{}", self.instance);
        let admin: AdminClient<DefaultClientContext> = self
            .base_config(&client_id)
            .create()
            .context("create the Kafka admin client")?;
        let metadata = admin
            .inner()
            .fetch_metadata(Some(topic), METADATA_TIMEOUT)
            .with_context(|| format!("read the metadata of {topic}"))?;
        let Some(described) = metadata
            .topics()
            .iter()
            .find(|described| described.name() == topic)
        else {
            return Ok(None);
        };
        if described.error().is_some() || described.partitions().is_empty() {
            return Ok(None);
        }
        let partitions = described.partitions().len() as u32;
        let options = AdminOptions::new().request_timeout(Some(METADATA_TIMEOUT));
        let results = futures::executor::block_on(
            admin.describe_configs([&ResourceSpecifier::Topic(topic)], &options),
        )
        .with_context(|| format!("describe the configuration of {topic}"))?;
        let cleanup_policy = results
            .into_iter()
            .flatten()
            .flat_map(|resource| resource.entries)
            .find(|entry| entry.name == "cleanup.policy")
            .and_then(|entry| entry.value)
            .unwrap_or_else(|| "delete".to_string());
        Ok(Some(TopicDescription {
            partitions,
            cleanup_policy,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_keys_like_the_java_default_partitioner() {
        assert_eq!(murmur2(b"21"), -973_932_308);
        assert_eq!(murmur2(b"foobar"), -790_332_482);
        assert_eq!(murmur2(b"a-little-bit-long-string"), -985_981_536);
        assert_eq!(murmur2(b"a-little-bit-longer-string"), -1_486_304_829);
        assert_eq!(
            murmur2(b"lkjh234lh9fiuh90y23oiuhsafujhadof229phr9h19h89h8"),
            -58_897_971
        );
        assert_eq!(murmur2(b"abc"), 479_470_107);
        assert!((0..6).contains(&default_partition("nightfall-0", 6)));
    }

    #[test]
    fn the_memory_broker_keeps_partition_order_and_wakes_consumers() {
        let broker = MemoryBroker::new();
        broker.create_topic("dusk.census", 3, "compact");
        let producer = broker.producer("census").unwrap();
        assert_eq!(producer.partition_count("dusk.census").unwrap(), 3);
        for index in 0..3 {
            futures::executor::block_on(producer.send(OutgoingRecord {
                topic: "dusk.census".to_string(),
                partition: Some(1),
                key: Some(format!("key-{index}")),
                payload: Some(vec![index]),
            }))
            .unwrap();
        }
        let mut consumer = broker
            .consumer("census", "dusk.census", Start::Beginning)
            .unwrap();
        for index in 0..3u8 {
            let record = consumer.poll(Duration::from_millis(10)).unwrap().unwrap();
            assert_eq!(record.partition, 1);
            assert_eq!(record.offset, i64::from(index));
            assert_eq!(record.payload, Some(vec![index]));
        }
        assert!(consumer.poll(Duration::from_millis(10)).unwrap().is_none());
        let watermarks = consumer.watermarks().unwrap();
        assert_eq!(
            watermarks.iter().map(|mark| mark.high).collect::<Vec<_>>(),
            vec![0, 3, 0]
        );
        assert!(caught_up(&watermarks, &consumer.positions().unwrap()));
        let waker = broker.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            waker
                .produce(OutgoingRecord {
                    topic: "dusk.census".to_string(),
                    partition: Some(0),
                    key: Some("late".to_string()),
                    payload: None,
                })
                .unwrap();
        });
        let late = consumer.poll(Duration::from_secs(5)).unwrap().unwrap();
        assert_eq!(late.key.as_deref(), Some(&b"late"[..]));
        assert!(late.payload.is_none());
        thread.join().unwrap();
        broker.fail_topic("dusk.census", Some("broker down"));
        assert!(
            futures::executor::block_on(producer.send(OutgoingRecord {
                topic: "dusk.census".to_string(),
                partition: None,
                key: None,
                payload: None,
            }))
            .is_err()
        );
        assert_eq!(
            broker
                .describe("dusk.census")
                .unwrap()
                .unwrap()
                .cleanup_policy,
            "compact"
        );
        assert!(broker.describe("dusk.missing").unwrap().is_none());
    }

    #[test]
    fn is_caught_up_once_every_partition_reached_its_high_watermark() {
        let targets = [
            Watermarks {
                partition: 0,
                low: 0,
                high: 5,
            },
            Watermarks {
                partition: 1,
                low: 7,
                high: 7,
            },
        ];
        assert!(!caught_up(&targets, &[(0, Some(4)), (1, None)]));
        assert!(!caught_up(&targets, &[(0, None), (1, None)]));
        assert!(caught_up(&targets, &[(0, Some(5)), (1, None)]));
        assert!(caught_up(&targets, &[(0, Some(9))]));
        assert!(!caught_up(&targets, &[(1, Some(9))]));
    }

    #[test]
    fn creates_a_producer_configuration_librdkafka_accepts() {
        let broker = KafkaBroker::new("127.0.0.1:1", &BTreeMap::new(), "nightfall-0");
        let producer: Result<FutureProducer, _> = broker.producer_config("census").create();
        assert!(producer.is_ok());
    }
}
