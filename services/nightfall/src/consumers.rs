use crate::backoff::full_jitter;
use crate::census::CensusRecord;
use crate::contracts::{Contract, ContractValidator};
use crate::kafka::{
    Broker, IncomingRecord, RecordConsumer, Start, Watermarks, caught_up, unix_milliseconds,
};
use crate::node_state::Change;
use crate::server::Shared;
use crate::shard::Control;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub const POLL: Duration = Duration::from_millis(200);
pub const IDLE_POLL: Duration = Duration::from_millis(50);
pub const RETRY_BASE: Duration = Duration::from_secs(1);
pub const RETRY_CAP: Duration = Duration::from_secs(60);
pub const CONNECTIONS_REWIND: Duration = Duration::from_secs(60);

fn invalid(record: &IncomingRecord, reason: &str) {
    metrics::counter!("nightfall_invalid_messages_total", "topic" => record.topic.clone())
        .increment(1);
    tracing::warn!(
        topic = %record.topic,
        partition = record.partition,
        offset = record.offset,
        reason,
        "dropped an invalid message"
    );
}

fn sleep_or_stop(stop: &CancellationToken, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        if stop.is_cancelled() {
            return true;
        }
        std::thread::sleep((deadline - Instant::now()).min(POLL));
    }
    stop.is_cancelled()
}

fn open(
    broker: &dyn Broker,
    role: &str,
    topic: &str,
    start: Start,
    stop: &CancellationToken,
) -> Option<(Box<dyn RecordConsumer>, Vec<Watermarks>)> {
    let mut attempt = 0u32;
    loop {
        let opened = broker
            .consumer(role, topic, start)
            .and_then(|mut consumer| {
                let targets = consumer
                    .watermarks()
                    .map_err(|error| anyhow::anyhow!("read the watermarks of {topic}: {error}"))?;
                Ok((consumer, targets))
            });
        match opened {
            Ok(opened) => return Some(opened),
            Err(error) => {
                attempt = attempt.saturating_add(1);
                tracing::error!(
                    topic,
                    role,
                    attempt,
                    error = format!("{error:#}"),
                    "a Kafka consumer could not start"
                );
                if sleep_or_stop(stop, full_jitter(attempt, RETRY_BASE, RETRY_CAP)) {
                    return None;
                }
            }
        }
    }
}

fn poll(
    consumer: &mut Box<dyn RecordConsumer>,
    topic: &str,
    timeout: Duration,
    failures: &mut u32,
    stop: &CancellationToken,
) -> Option<IncomingRecord> {
    match consumer.poll(timeout) {
        Ok(record) => {
            *failures = 0;
            record
        }
        Err(error) => {
            *failures = failures.saturating_add(1);
            metrics::counter!("nightfall_consumer_errors_total", "topic" => topic.to_string())
                .increment(1);
            tracing::warn!(topic, %error, failures = *failures, "polling Kafka failed");
            sleep_or_stop(
                stop,
                full_jitter(
                    *failures,
                    Duration::from_millis(100),
                    Duration::from_secs(10),
                ),
            );
            None
        }
    }
}

fn positions_reached(consumer: &mut Box<dyn RecordConsumer>, targets: &[Watermarks]) -> bool {
    match consumer.positions() {
        Ok(positions) => caught_up(targets, &positions),
        Err(error) => {
            tracing::debug!(%error, "consumer positions are not known yet");
            false
        }
    }
}

fn node_state(
    shared: Arc<Shared>,
    broker: Arc<dyn Broker>,
    validator: ContractValidator,
    stop: CancellationToken,
) {
    let topic = shared.config.kafka.topics.node_state.clone();
    let Some((mut consumer, targets)) = open(
        broker.as_ref(),
        "node-state",
        &topic,
        Start::Beginning,
        &stop,
    ) else {
        return;
    };
    let started = Instant::now();
    let mut ready = false;
    let mut failures = 0u32;
    let mut applied = 0u64;
    while !stop.is_cancelled() {
        let record = poll(&mut consumer, &topic, POLL, &mut failures, &stop);
        if let Some(record) = record {
            let checked = match &record.payload {
                Some(payload) => validator.check(payload).map(|_| ()),
                None => Ok(()),
            };
            match checked.and_then(|()| {
                crate::node_state::parse(record.key.as_deref(), record.payload.as_deref())
            }) {
                Ok(state) => {
                    applied += 1;
                    let flip = shared.node_states.apply(state);
                    if ready && flip.change != Change::Unchanged {
                        tracing::info!(scope = ?flip.scope, change = ?flip.change, "node state changed");
                        shared.flip(flip.scope);
                    }
                }
                Err(reason) => invalid(&record, &reason),
            }
            if let Some(timestamp) = record.timestamp_ms {
                let lag = (unix_milliseconds() - timestamp).max(0) as f64 / 1000.0;
                metrics::gauge!("nightfall_node_state_lag_seconds").set(lag);
            }
        } else if ready {
            metrics::gauge!("nightfall_node_state_lag_seconds").set(0.0);
        }
        if !ready && positions_reached(&mut consumer, &targets) {
            ready = true;
            shared.readiness.node_state.store(true, Ordering::Release);
            shared.broadcast(|| Control::Recheck);
            tracing::info!(
                records = applied,
                states = shared.node_states.len(),
                seconds = started.elapsed().as_secs_f64(),
                "node state caught up"
            );
        }
    }
}

struct DirectoryConsumer {
    shared: Arc<Shared>,
    census: ContractValidator,
    connections: ContractValidator,
}

impl DirectoryConsumer {
    fn census_record(&self, record: &IncomingRecord) {
        if let Some(payload) = &record.payload
            && let Err(reason) = self.census.check(payload)
        {
            invalid(record, &reason);
            return;
        }
        let parsed = match crate::census::parse(record.payload.as_deref()) {
            Ok(parsed) => parsed,
            Err(reason) => {
                invalid(record, &reason);
                return;
            }
        };
        let replaced = {
            let mut directory = self.shared.directory();
            match parsed {
                CensusRecord::Header(header) => {
                    let header_ms = crate::events::parse_time_milliseconds(&header.time)
                        .or(record.timestamp_ms)
                        .unwrap_or_else(unix_milliseconds);
                    directory.apply_census_header(header, header_ms)
                }
                CensusRecord::Chunk(chunk) => {
                    directory.apply_census_chunk(chunk);
                    Vec::new()
                }
                CensusRecord::Tombstone => Vec::new(),
            }
        };
        for replacement in replaced {
            self.replace(replacement);
        }
    }

    fn connection_record(&self, record: &IncomingRecord) {
        let Some(payload) = &record.payload else {
            invalid(record, "a connections record without a value");
            return;
        };
        if let Err(reason) = self.connections.check(payload) {
            invalid(record, &reason);
            return;
        }
        let event = match crate::events::parse_connection(payload) {
            Ok(event) => event,
            Err(reason) => {
                invalid(record, &reason);
                return;
            }
        };
        let replaced = self.shared.directory().apply_connection(&event);
        if let Some(replacement) = replaced {
            self.replace(replacement);
        }
    }

    fn replace(&self, replaced: crate::directory::Replaced) {
        tracing::info!(
            namespace_id = %crate::directory::namespace_hex(replaced.namespace_id),
            epoch = replaced.epoch,
            newer_epoch = replaced.newer_epoch,
            instance = %replaced.instance,
            "a newer session of this node opened on another instance"
        );
        self.shared.send(
            replaced.shard,
            Control::Close {
                namespace_id: replaced.namespace_id,
                epoch: replaced.epoch,
                reason: crate::events::DisconnectReason::Replaced,
            },
        );
    }

    fn expire(&self) {
        let dead = self
            .shared
            .directory()
            .expire_instances(unix_milliseconds());
        if !dead.is_empty() {
            metrics::counter!("nightfall_dead_instances_total").increment(dead.len() as u64);
        }
    }
}

fn directory(
    shared: Arc<Shared>,
    broker: Arc<dyn Broker>,
    consumer: DirectoryConsumer,
    stop: CancellationToken,
) {
    let census_topic = shared.config.kafka.topics.census.clone();
    let connections_topic = shared.config.kafka.topics.connections.clone();
    let Some((mut census, targets)) = open(
        broker.as_ref(),
        "census",
        &census_topic,
        Start::Beginning,
        &stop,
    ) else {
        return;
    };
    let started = Instant::now();
    let mut failures = 0u32;
    while !stop.is_cancelled() && !positions_reached(&mut census, &targets) {
        if let Some(record) = poll(&mut census, &census_topic, POLL, &mut failures, &stop) {
            consumer.census_record(&record);
        }
    }
    if stop.is_cancelled() {
        return;
    }
    consumer.expire();
    let oldest = {
        let mut directory = shared.directory();
        directory.seed_epoch(crate::directory::unix_microseconds());
        directory.oldest_census_ms()
    }
    .unwrap_or_else(unix_milliseconds)
    .min(unix_milliseconds());
    let rewind = oldest - CONNECTIONS_REWIND.as_millis() as i64;
    let Some((mut connections, _)) = open(
        broker.as_ref(),
        "connections",
        &connections_topic,
        Start::TimeMilliseconds(rewind),
        &stop,
    ) else {
        return;
    };
    shared.readiness.directory.store(true, Ordering::Release);
    tracing::info!(
        seconds = started.elapsed().as_secs_f64(),
        connections_from = %crate::events::format_unix_milliseconds(rewind),
        remote = shared.directory().remote_count(),
        "directory caught up with the census"
    );
    let heartbeat = Duration::from_secs(shared.config.kafka.census_heartbeat_seconds);
    let mut last_expiry = Instant::now();
    let mut census_failures = 0u32;
    let mut connection_failures = 0u32;
    while !stop.is_cancelled() {
        let census_record = poll(
            &mut census,
            &census_topic,
            Duration::ZERO,
            &mut census_failures,
            &stop,
        );
        let connection_record = poll(
            &mut connections,
            &connections_topic,
            Duration::ZERO,
            &mut connection_failures,
            &stop,
        );
        let idle = census_record.is_none() && connection_record.is_none();
        if let Some(record) = census_record {
            consumer.census_record(&record);
        }
        if let Some(record) = connection_record {
            consumer.connection_record(&record);
        }
        if idle
            && let Some(record) = poll(
                &mut connections,
                &connections_topic,
                IDLE_POLL,
                &mut connection_failures,
                &stop,
            )
        {
            consumer.connection_record(&record);
        }
        if last_expiry.elapsed() >= heartbeat {
            last_expiry = Instant::now();
            consumer.expire();
        }
    }
}

pub fn start(
    shared: Arc<Shared>,
    broker: Arc<dyn Broker>,
    stop: CancellationToken,
) -> anyhow::Result<Vec<std::thread::JoinHandle<()>>> {
    let node_state_validator = Contract::NodeState.validator()?;
    let directory_consumer = DirectoryConsumer {
        shared: shared.clone(),
        census: Contract::Census.validator()?,
        connections: Contract::Connections.validator()?,
    };
    let node_state_thread = {
        let shared = shared.clone();
        let broker = broker.clone();
        let stop = stop.clone();
        std::thread::Builder::new()
            .name("nightfall-node-state".to_string())
            .spawn(move || node_state(shared, broker, node_state_validator, stop))?
    };
    let directory_thread = std::thread::Builder::new()
        .name("nightfall-directory".to_string())
        .spawn(move || directory(shared, broker, directory_consumer, stop))?;
    Ok(vec![node_state_thread, directory_thread])
}
