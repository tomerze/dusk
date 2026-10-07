use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use rdkafka::ClientConfig;
use rdkafka::consumer::{BaseConsumer, Consumer};
use rdkafka::error::{KafkaError, RDKafkaErrorCode};
use rdkafka::message::Message;
use rdkafka::producer::{BaseRecord, DefaultProducerContext, Producer, ThreadedProducer};
use rdkafka::topic_partition_list::{Offset, TopicPartitionList};
use tracing::{info, warn};

use crate::log::{LedgerLog, LogError, LogErrorKind};

const QUEUE_FULL_WAIT: Duration = Duration::from_millis(5);

#[derive(Debug, Clone)]
pub struct KafkaLogConfig {
    pub brokers: String,
    pub properties: BTreeMap<String, String>,
    pub topic: String,
    pub partition: u32,
    pub instance: String,
    pub operation_timeout: Duration,
}

impl KafkaLogConfig {
    pub fn transactional_id(&self) -> String {
        format!("nightfall-ledger-{}", self.instance)
    }

    pub fn producer_config(&self) -> ClientConfig {
        let mut config = ClientConfig::new();
        for (name, value) in &self.properties {
            config.set(name, value);
        }
        config
            .set("bootstrap.servers", &self.brokers)
            .set("client.id", self.transactional_id())
            .set("transactional.id", self.transactional_id())
            .set("enable.idempotence", "true")
            .set("acks", "all")
            .set("compression.type", "zstd")
            .set("linger.ms", "5")
            .set("message.timeout.ms", "0")
            .set("enable.gapless.guarantee", "true")
            .set("allow.auto.create.topics", "false");
        config
    }

    pub fn consumer_config(&self) -> ClientConfig {
        reader_config(
            &self.brokers,
            &self.properties,
            &format!("nightfall-ledger-reader-{}", self.instance),
        )
    }
}

pub fn reader_config(
    brokers: &str,
    properties: &BTreeMap<String, String>,
    client_id: &str,
) -> ClientConfig {
    let mut config = ClientConfig::new();
    for (name, value) in properties {
        config.set(name, value);
    }
    config
        .set("bootstrap.servers", brokers)
        .set("client.id", client_id)
        .set("group.id", client_id)
        .set("isolation.level", "read_committed")
        .set("enable.partition.eof", "true")
        .set("enable.auto.commit", "false")
        .set("enable.auto.offset.store", "false")
        .set("auto.offset.reset", "earliest")
        .set("allow.auto.create.topics", "false");
    config
}

pub struct KafkaLog {
    config: KafkaLogConfig,
    key: String,
    producer: ThreadedProducer<DefaultProducerContext>,
    consumer: BaseConsumer,
}

impl KafkaLog {
    pub fn connect(config: KafkaLogConfig) -> Result<KafkaLog, LogError> {
        let consumer: BaseConsumer = config.consumer_config().create().map_err(|error| {
            LogError::new(
                LogErrorKind::Fatal,
                format!("creating the ledger reader failed: {error}"),
            )
        })?;
        check_partition_count(&consumer, &config)?;
        let producer = create_producer(&config)?;
        let key = config.instance.clone();
        info!(
            topic = %config.topic,
            partition = config.partition,
            transactional_id = %config.transactional_id(),
            "ledger producer initialized its transactions"
        );
        Ok(KafkaLog {
            config,
            key,
            producer,
            consumer,
        })
    }

    fn classify(&self, error: KafkaError) -> LogError {
        if let Some((code, reason)) = self.producer.client().fatal_error() {
            return LogError::new(fatal_kind(code), format!("{code:?}: {reason}"));
        }
        classify(error)
    }
}

fn create_producer(
    config: &KafkaLogConfig,
) -> Result<ThreadedProducer<DefaultProducerContext>, LogError> {
    let producer: ThreadedProducer<DefaultProducerContext> =
        config.producer_config().create().map_err(|error| {
            LogError::new(
                LogErrorKind::Fatal,
                format!("creating the ledger producer failed: {error}"),
            )
        })?;
    producer
        .init_transactions(config.operation_timeout)
        .map_err(classify)?;
    Ok(producer)
}

fn check_partition_count(consumer: &BaseConsumer, config: &KafkaLogConfig) -> Result<(), LogError> {
    let metadata = consumer
        .fetch_metadata(Some(&config.topic), config.operation_timeout)
        .map_err(|error| {
            LogError::new(
                LogErrorKind::Retriable,
                format!("reading the {} metadata failed: {error}", config.topic),
            )
        })?;
    let Some(topic) = metadata
        .topics()
        .iter()
        .find(|topic| topic.name() == config.topic)
    else {
        return Err(LogError::new(
            LogErrorKind::Fatal,
            format!("the broker reports no topic {}", config.topic),
        ));
    };
    if let Some(error) = topic.error() {
        return Err(LogError::new(
            LogErrorKind::Fatal,
            format!(
                "the topic {} is not available: {:?}",
                config.topic,
                RDKafkaErrorCode::from(error)
            ),
        ));
    }
    let partitions = topic.partitions().len();
    if partitions <= config.partition as usize {
        return Err(LogError::new(
            LogErrorKind::Fatal,
            format!(
                "the topic {} has {partitions} partitions; ledger partition {} needs at least {}",
                config.topic,
                config.partition,
                config.partition + 1
            ),
        ));
    }
    Ok(())
}

fn fatal_kind(code: RDKafkaErrorCode) -> LogErrorKind {
    match code {
        RDKafkaErrorCode::Fenced
        | RDKafkaErrorCode::ProducerFenced
        | RDKafkaErrorCode::InvalidProducerEpoch => LogErrorKind::Fenced,
        _ => LogErrorKind::Fatal,
    }
}

fn classify(error: KafkaError) -> LogError {
    let message = error.to_string();
    let kind = match &error {
        KafkaError::Transaction(transaction_error) => {
            if transaction_error.is_fatal() {
                fatal_kind(transaction_error.code())
            } else if transaction_error.txn_requires_abort() {
                LogErrorKind::Abortable
            } else if transaction_error.is_retriable() {
                LogErrorKind::Retriable
            } else {
                fatal_kind(transaction_error.code())
            }
        }
        KafkaError::MessageProduction(RDKafkaErrorCode::Fatal) => LogErrorKind::Fatal,
        KafkaError::MessageProduction(code) => match fatal_kind(*code) {
            LogErrorKind::Fenced => LogErrorKind::Fenced,
            _ => LogErrorKind::Abortable,
        },
        _ => LogErrorKind::Abortable,
    };
    LogError::new(kind, message)
}

impl LedgerLog for KafkaLog {
    fn begin(&mut self) -> Result<(), LogError> {
        self.producer
            .begin_transaction()
            .map_err(|error| self.classify(error))
    }

    fn append(&mut self, payload: &[u8]) -> Result<(), LogError> {
        let mut record: BaseRecord<'_, String, [u8]> = BaseRecord::to(&self.config.topic)
            .key(&self.key)
            .payload(payload)
            .partition(self.config.partition as i32);
        let deadline = Instant::now() + self.config.operation_timeout;
        loop {
            match self.producer.send(record) {
                Ok(()) => return Ok(()),
                Err((KafkaError::MessageProduction(RDKafkaErrorCode::QueueFull), returned)) => {
                    if Instant::now() >= deadline {
                        return Err(LogError::new(
                            LogErrorKind::Abortable,
                            "the librdkafka queue stayed full for the whole operation timeout",
                        ));
                    }
                    record = returned;
                    std::thread::sleep(QUEUE_FULL_WAIT);
                }
                Err((error, _)) => return Err(self.classify(error)),
            }
        }
    }

    fn commit(&mut self) -> Result<(), LogError> {
        self.producer
            .commit_transaction(self.config.operation_timeout)
            .map_err(|error| self.classify(error))
    }

    fn abort(&mut self) -> Result<(), LogError> {
        self.producer
            .abort_transaction(self.config.operation_timeout)
            .map_err(|error| self.classify(error))
    }

    fn read_tail(&mut self, count: usize) -> Result<Vec<Vec<u8>>, LogError> {
        let timeout = self.config.operation_timeout;
        let (low, high) = self
            .consumer
            .fetch_watermarks(&self.config.topic, self.config.partition as i32, timeout)
            .map_err(|error| {
                LogError::new(
                    LogErrorKind::Retriable,
                    format!("reading the ledger watermarks failed: {error}"),
                )
            })?;
        if count == 0 || high <= low {
            return Ok(Vec::new());
        }
        let mut window = (count as i64).saturating_add(8).saturating_mul(2);
        loop {
            let start = high.saturating_sub(window).max(low);
            let mut tail = VecDeque::with_capacity(count);
            read_partition(
                &self.consumer,
                &self.config.topic,
                self.config.partition,
                start,
                Some(high),
                timeout,
                |_, payload| {
                    if tail.len() == count {
                        tail.pop_front();
                    }
                    tail.push_back(payload.to_vec());
                },
            )?;
            if tail.len() == count || start == low {
                return Ok(tail.into());
            }
            window = window.saturating_mul(4);
        }
    }

    fn recover(&mut self) -> Result<(), LogError> {
        warn!(transactional_id = %self.config.transactional_id(), "recreating the ledger producer");
        self.producer = create_producer(&self.config)?;
        Ok(())
    }
}

pub fn read_partition(
    consumer: &BaseConsumer,
    topic: &str,
    partition: u32,
    start: i64,
    end: Option<i64>,
    timeout: Duration,
    mut on_record: impl FnMut(i64, &[u8]),
) -> Result<u64, LogError> {
    let mut assignment = TopicPartitionList::new();
    assignment
        .add_partition_offset(topic, partition as i32, Offset::Offset(start))
        .map_err(|error| {
            LogError::new(
                LogErrorKind::Fatal,
                format!("assigning offset {start} failed: {error}"),
            )
        })?;
    consumer.assign(&assignment).map_err(|error| {
        LogError::new(
            LogErrorKind::Retriable,
            format!("assigning the ledger partition failed: {error}"),
        )
    })?;
    let mut records = 0u64;
    let mut deadline = Instant::now() + timeout;
    let outcome = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break Err(LogError::new(
                LogErrorKind::Retriable,
                format!(
                    "reading {topic} partition {partition} made no progress within {timeout:?}"
                ),
            ));
        }
        match consumer.poll(remaining) {
            None => continue,
            Some(Err(KafkaError::PartitionEOF(_))) => break Ok(records),
            Some(Err(error)) => {
                break Err(LogError::new(
                    LogErrorKind::Retriable,
                    format!("reading {topic} partition {partition} failed: {error}"),
                ));
            }
            Some(Ok(message)) => {
                if end.is_some_and(|end| message.offset() >= end) {
                    break Ok(records);
                }
                if let Some(payload) = message.payload() {
                    on_record(message.offset(), payload);
                    records += 1;
                }
                deadline = Instant::now() + timeout;
                if end.is_some_and(|end| message.offset() + 1 >= end) {
                    break Ok(records);
                }
            }
        }
    };
    if let Err(error) = consumer.unassign() {
        warn!(error = %error, "unassigning the ledger partition failed");
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> KafkaLogConfig {
        KafkaLogConfig {
            brokers: String::from("127.0.0.1:1"),
            properties: BTreeMap::from([(String::from("acks"), String::from("1"))]),
            topic: String::from("dusk.ledger"),
            partition: 0,
            instance: String::from("nightfall-0"),
            operation_timeout: Duration::from_secs(1),
        }
    }

    #[test]
    fn creates_the_transactional_zstd_producer() {
        let config = config();
        let client_config = config.producer_config();
        assert_eq!(
            client_config.get("transactional.id"),
            Some("nightfall-ledger-nightfall-0")
        );
        assert_eq!(client_config.get("enable.idempotence"), Some("true"));
        assert_eq!(client_config.get("compression.type"), Some("zstd"));
        assert_eq!(client_config.get("acks"), Some("all"));
        assert_eq!(client_config.get("message.timeout.ms"), Some("0"));
        assert_eq!(client_config.get("enable.gapless.guarantee"), Some("true"));
        let producer: ThreadedProducer<DefaultProducerContext> = client_config.create().unwrap();
        drop(producer);
    }

    #[test]
    fn creates_the_read_committed_reader() {
        let client_config = config().consumer_config();
        assert_eq!(client_config.get("isolation.level"), Some("read_committed"));
        let consumer: BaseConsumer = client_config.create().unwrap();
        drop(consumer);
    }

    #[test]
    fn classifies_fencing_as_fenced() {
        assert_eq!(
            fatal_kind(RDKafkaErrorCode::ProducerFenced),
            LogErrorKind::Fenced
        );
        assert_eq!(fatal_kind(RDKafkaErrorCode::Fenced), LogErrorKind::Fenced);
        assert_eq!(
            fatal_kind(RDKafkaErrorCode::BrokerNotAvailable),
            LogErrorKind::Fatal
        );
        assert_eq!(
            classify(KafkaError::MessageProduction(RDKafkaErrorCode::Fatal)).kind,
            LogErrorKind::Fatal
        );
        assert_eq!(
            classify(KafkaError::MessageProduction(
                RDKafkaErrorCode::MessageSizeTooLarge
            ))
            .kind,
            LogErrorKind::Abortable
        );
    }
}
