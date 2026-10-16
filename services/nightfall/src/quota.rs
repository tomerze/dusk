use crate::kafka::{OutgoingRecord, RecordProducer};
use nightfall_provisioning::credential::{CredentialKind, CredentialName};
use nightfall_provisioning::quota::{
    InstallationQuota, QuotaUnavailable, Reservation, ReservationOutcome,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::oneshot;

pub const SCHEMA: &str = "dusk.credential-quota/v1";
pub const PARTITION: i32 = 0;
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(10);
pub const ABANDONED_CAPACITY: usize = 65_536;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuotaMessage {
    schema: String,
    id: String,
    time: String,
    operation: String,
    reservation: String,
    credential_kind: String,
    credential: String,
    limit: u64,
    device_id: String,
    installation_id: String,
    instance: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Reserve,
    Release,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotaRecord {
    pub operation: Operation,
    pub reservation: String,
    pub credential: CredentialName,
    pub limit: u64,
}

fn kind_named(name: &str) -> Option<CredentialKind> {
    match name {
        "fleet_token" => Some(CredentialKind::FleetToken),
        "install_token" => Some(CredentialKind::InstallToken),
        "provision_token" => Some(CredentialKind::ProvisionToken),
        _ => None,
    }
}

pub fn parse(payload: &[u8]) -> Result<QuotaRecord, String> {
    let message: QuotaMessage =
        serde_json::from_slice(payload).map_err(|error| error.to_string())?;
    if message.schema != SCHEMA {
        return Err(format!("schema {:?}", message.schema));
    }
    let operation = match message.operation.as_str() {
        "reserve" => Operation::Reserve,
        "release" => Operation::Release,
        other => return Err(format!("operation {other:?}")),
    };
    let kind = kind_named(&message.credential_kind)
        .ok_or_else(|| format!("credential kind {:?}", message.credential_kind))?;
    Ok(QuotaRecord {
        operation,
        reservation: message.reservation,
        credential: CredentialName {
            kind,
            name: message.credential,
        },
        limit: message.limit,
    })
}

fn encode(
    operation: Operation,
    reservation: &Reservation,
    instance: &str,
) -> Result<(String, Vec<u8>), String> {
    let (name, suffix) = match operation {
        Operation::Reserve => ("reserve", ""),
        Operation::Release => ("release", "/release"),
    };
    let id = match operation {
        Operation::Reserve => reservation.id.clone(),
        Operation::Release => uuid::Uuid::now_v7().to_string(),
    };
    let message = QuotaMessage {
        schema: SCHEMA.to_string(),
        id,
        time: crate::events::now(),
        operation: name.to_string(),
        reservation: reservation.id.clone(),
        credential_kind: reservation.credential.kind.name().to_string(),
        credential: reservation.credential.name.clone(),
        limit: reservation.limit,
        device_id: reservation.device_id.clone(),
        installation_id: reservation.installation_id.clone(),
        instance: instance.to_string(),
    };
    let key = format!(
        "{}/{}{suffix}",
        reservation.credential.key(),
        reservation.installation_id
    );
    serde_json::to_vec(&message)
        .map(|payload| (key, payload))
        .map_err(|error| error.to_string())
}

type Decision = Result<bool, String>;

#[derive(Default)]
struct State {
    ready: bool,
    used: HashMap<CredentialName, u64>,
    waiting: HashMap<String, (Reservation, oneshot::Sender<Decision>)>,
    abandoned: HashMap<String, Reservation>,
    records: u64,
}

pub struct KafkaQuota {
    instance: String,
    topic: String,
    producer: Arc<dyn RecordProducer>,
    runtime: tokio::runtime::Handle,
    confirm_timeout: Duration,
    state: Arc<Mutex<State>>,
}

fn outcome(name: &'static str) {
    metrics::counter!("nightfall_credential_quota_reservations_total", "outcome" => name)
        .increment(1);
}

impl KafkaQuota {
    pub fn new(
        instance: &str,
        topic: &str,
        producer: Arc<dyn RecordProducer>,
        runtime: tokio::runtime::Handle,
        confirm_timeout: Duration,
    ) -> KafkaQuota {
        KafkaQuota {
            instance: instance.to_string(),
            topic: topic.to_string(),
            producer,
            runtime,
            confirm_timeout,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn set_ready(&self) {
        let mut state = self.state();
        state.ready = true;
        tracing::info!(
            records = state.records,
            credentials = state.used.len(),
            installations = state.used.values().sum::<u64>(),
            "credential installation counts caught up"
        );
    }

    pub fn snapshot(&self) -> Option<HashMap<CredentialName, u64>> {
        let state = self.state();
        state.ready.then(|| state.used.clone())
    }

    pub fn apply(&self, record: QuotaRecord) {
        let mut state = self.state();
        state.records += 1;
        match record.operation {
            Operation::Reserve => {
                let used = state.used.entry(record.credential.clone()).or_default();
                let granted = *used < record.limit;
                if granted {
                    *used += 1;
                }
                let used = *used;
                let given_back = match state.waiting.remove(&record.reservation) {
                    Some((reservation, waiter)) => {
                        if waiter.send(Ok(granted)).is_err() && granted {
                            Some(reservation)
                        } else {
                            None
                        }
                    }
                    None => state
                        .abandoned
                        .remove(&record.reservation)
                        .filter(|_| granted),
                };
                if state.ready {
                    tracing::debug!(
                        credential_kind = record.credential.kind.name(),
                        credential = %record.credential.name,
                        reservation = %record.reservation,
                        granted,
                        used,
                        limit = record.limit,
                        "an installation reservation was read"
                    );
                }
                drop(state);
                if let Some(reservation) = given_back {
                    tracing::warn!(
                        credential_kind = reservation.credential.kind.name(),
                        credential = %reservation.credential.name,
                        reservation = %reservation.id,
                        "a granted reservation nobody waits for any more is given back"
                    );
                    self.release(&reservation);
                }
            }
            Operation::Release => {
                if let Some(used) = state.used.get_mut(&record.credential) {
                    *used = used.saturating_sub(1);
                }
            }
        }
    }

    fn write(&self, operation: Operation, reservation: &Reservation) -> crate::kafka::Delivery {
        match encode(operation, reservation, &self.instance) {
            Ok((key, payload)) => self.producer.send(OutgoingRecord {
                topic: self.topic.clone(),
                partition: Some(PARTITION),
                key: Some(key),
                payload: Some(payload),
            }),
            Err(error) => Box::pin(async move { Err(error) }),
        }
    }
}

impl InstallationQuota for KafkaQuota {
    fn used(&self, credential: &CredentialName) -> Result<u64, QuotaUnavailable> {
        let state = self.state();
        if !state.ready {
            return Err(QuotaUnavailable(
                "the installations of capped credentials are still being counted".to_string(),
            ));
        }
        Ok(state.used.get(credential).copied().unwrap_or(0))
    }

    fn reserve(&self, reservation: Reservation) -> ReservationOutcome {
        let (sender, mut receiver) = oneshot::channel();
        {
            let mut state = self.state();
            if !state.ready {
                return Box::pin(async {
                    Err(QuotaUnavailable(
                        "the installations of capped credentials are still being counted"
                            .to_string(),
                    ))
                });
            }
            state
                .waiting
                .insert(reservation.id.clone(), (reservation.clone(), sender));
        }
        let delivery = self.write(Operation::Reserve, &reservation);
        let identifier = reservation.id.clone();
        let written = self.state.clone();
        self.runtime.spawn(async move {
            let Err(error) = delivery.await else {
                return;
            };
            let waiting = {
                let mut state = written
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.abandoned.remove(&identifier);
                state.waiting.remove(&identifier)
            };
            let told = waiting.is_some_and(|(_, waiter)| waiter.send(Err(error.clone())).is_ok());
            if !told {
                tracing::warn!(
                    reservation = %identifier,
                    %error,
                    "a reservation nobody waits for any more was not written; it counts nothing"
                );
            }
        });
        let state = self.state.clone();
        let confirm_timeout = self.confirm_timeout;
        Box::pin(async move {
            let decided = |decision: Decision| match decision {
                Ok(granted) => {
                    outcome(if granted { "granted" } else { "refused" });
                    Ok(granted)
                }
                Err(error) => {
                    outcome("unwritten");
                    Err(QuotaUnavailable(format!(
                        "the reservation was not written: {error}"
                    )))
                }
            };
            match tokio::time::timeout(confirm_timeout, &mut receiver).await {
                Ok(Ok(decision)) => decided(decision),
                Ok(Err(_)) => {
                    outcome("unconfirmed");
                    Err(QuotaUnavailable(
                        "the reservation was dropped before it was read".to_string(),
                    ))
                }
                Err(_) => {
                    let mut locked = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if locked.waiting.remove(&reservation.id).is_some() {
                        if locked.abandoned.len() < ABANDONED_CAPACITY {
                            locked
                                .abandoned
                                .insert(reservation.id.clone(), reservation.clone());
                        } else {
                            tracing::warn!(
                                reservation = %reservation.id,
                                "too many reservations are unconfirmed; if this one is granted, its installation stays counted"
                            );
                        }
                        drop(locked);
                        outcome("unconfirmed");
                        return Err(QuotaUnavailable(format!(
                            "the reservation was not written and read back within {} ms",
                            confirm_timeout.as_millis()
                        )));
                    }
                    drop(locked);
                    match receiver.try_recv() {
                        Ok(decision) => decided(decision),
                        Err(_) => {
                            outcome("unconfirmed");
                            Err(QuotaUnavailable(
                                "the reservation was dropped before it was read".to_string(),
                            ))
                        }
                    }
                }
            }
        })
    }

    fn release(&self, reservation: &Reservation) {
        let delivery = self.write(Operation::Release, reservation);
        let credential = reservation.credential.clone();
        let identifier = reservation.id.clone();
        self.runtime.spawn(async move {
            if let Err(error) = delivery.await {
                metrics::counter!("nightfall_credential_quota_release_failures_total").increment(1);
                tracing::error!(
                    credential_kind = credential.kind.name(),
                    credential = %credential.name,
                    reservation = %identifier,
                    %error,
                    "an installation reservation was not given back; the installation stays counted"
                );
            }
        });
    }

    fn record(&self, reservation: &Reservation) {
        let delivery = self.write(Operation::Reserve, reservation);
        let credential = reservation.credential.clone();
        let installation_id = reservation.installation_id.clone();
        self.runtime.spawn(async move {
            if let Err(error) = delivery.await {
                metrics::counter!("nightfall_credential_quota_record_failures_total").increment(1);
                tracing::error!(
                    credential_kind = credential.kind.name(),
                    credential = %credential.name,
                    installation_id = %installation_id,
                    %error,
                    "an installation of an uncapped credential was not recorded; a cap set on the credential later does not count it"
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::Contract;
    use crate::kafka::{Broker, MemoryBroker};

    const TOPIC: &str = "dusk.credential-quota";

    fn credential(name: &str) -> CredentialName {
        CredentialName {
            kind: CredentialKind::FleetToken,
            name: name.to_string(),
        }
    }

    fn reservation(name: &str, installation: u8, limit: u64) -> Reservation {
        Reservation {
            id: uuid::Uuid::now_v7().to_string(),
            credential: credential(name),
            limit,
            device_id: "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13".to_string(),
            installation_id: format!("{installation:032x}"),
        }
    }

    fn quota(broker: &Arc<MemoryBroker>, confirm_timeout: Duration) -> Arc<KafkaQuota> {
        broker.create_topic(TOPIC, 3, "compact");
        Arc::new(KafkaQuota::new(
            "nightfall-0",
            TOPIC,
            broker.producer("quota").unwrap(),
            tokio::runtime::Handle::current(),
            confirm_timeout,
        ))
    }

    fn apply_new(broker: &MemoryBroker, quota: &KafkaQuota, read: &mut usize) {
        let records = broker.records(TOPIC);
        let validator = Contract::CredentialQuota.validator().unwrap();
        let mut ordered: Vec<_> = records.into_iter().collect();
        ordered.sort_by_key(|record| (record.partition, record.offset));
        for record in ordered.iter().skip(*read) {
            let payload = record.payload.as_deref().unwrap();
            validator.check(payload).unwrap();
            quota.apply(parse(payload).unwrap());
        }
        *read = ordered.len();
    }

    #[test]
    fn writes_reservations_and_releases_its_contract_accepts() {
        let validator = Contract::CredentialQuota.validator().unwrap();
        let reserved = reservation("retail-eu-2026", 1, 50_000);
        for operation in [Operation::Reserve, Operation::Release] {
            let (key, payload) = encode(operation, &reserved, "nightfall-0").unwrap();
            validator.check(&payload).unwrap();
            let parsed = parse(&payload).unwrap();
            assert_eq!(parsed.operation, operation);
            assert_eq!(parsed.reservation, reserved.id);
            assert_eq!(parsed.credential, reserved.credential);
            assert_eq!(parsed.limit, 50_000);
            let expected = format!("fleet_token/retail-eu-2026/{:032x}", 1);
            match operation {
                Operation::Reserve => assert_eq!(key, expected),
                Operation::Release => assert_eq!(key, format!("{expected}/release")),
            }
        }
        assert!(parse(br#"{"schema":"dusk.credential-quota/v2"}"#).is_err());
    }

    #[test]
    fn reads_and_writes_a_provision_token_reservation() {
        let validator = Contract::CredentialQuota.validator().unwrap();
        let example = include_str!(
            "../../contracts/kafka/examples/dusk.credential-quota/reserve-provision-token.json"
        );
        let parsed = parse(example.as_bytes()).unwrap();
        let provision_token = CredentialName {
            kind: CredentialKind::ProvisionToken,
            name: "retail-eu-2026-10".to_string(),
        };
        assert_eq!(parsed.operation, Operation::Reserve);
        assert_eq!(parsed.credential, provision_token);
        assert_eq!(parsed.limit, 50_000);
        let reserved = Reservation {
            credential: provision_token.clone(),
            ..reservation("retail-eu-2026-10", 2, 50_000)
        };
        let (key, payload) = encode(Operation::Reserve, &reserved, "nightfall-0").unwrap();
        validator.check(&payload).unwrap();
        assert_eq!(parse(&payload).unwrap().credential, provision_token);
        assert_eq!(key, format!("provision_token/retail-eu-2026-10/{:032x}", 2));
    }

    #[tokio::test]
    async fn every_instance_counts_the_same_winner_in_log_order() {
        let broker = MemoryBroker::new();
        let quota = quota(&broker, CONFIRM_TIMEOUT);
        let watcher = KafkaQuota::new(
            "nightfall-1",
            TOPIC,
            broker.producer("quota").unwrap(),
            tokio::runtime::Handle::current(),
            CONFIRM_TIMEOUT,
        );
        assert!(quota.used(&credential("lab")).is_err());
        assert!(
            quota
                .reserve(reservation("lab", 1, 1))
                .await
                .unwrap_err()
                .0
                .contains("still being counted")
        );
        for counting in [quota.as_ref(), &watcher] {
            counting.set_ready();
        }
        let first = quota.reserve(reservation("lab", 1, 1));
        let second = quota.reserve(reservation("lab", 2, 1));
        let other = quota.reserve(reservation("other", 3, 1));
        let mut read = 0;
        let mut watched = 0;
        let decisions = tokio::join!(first, second, other, async {
            tokio::task::yield_now().await;
            apply_new(&broker, &quota, &mut read);
            apply_new(&broker, &watcher, &mut watched);
        });
        assert_eq!(
            (decisions.0, decisions.1, decisions.2),
            (Ok(true), Ok(false), Ok(true))
        );
        assert_eq!(quota.used(&credential("lab")), Ok(1));
        assert_eq!(watcher.used(&credential("lab")), Ok(1));
        assert_eq!(watcher.used(&credential("other")), Ok(1));
        assert_eq!(watcher.used(&credential("unused")), Ok(0));
        let granted = reservation("lab", 1, 1);
        quota.release(&granted);
        tokio::task::yield_now().await;
        apply_new(&broker, &quota, &mut read);
        apply_new(&broker, &watcher, &mut watched);
        assert_eq!(quota.used(&credential("lab")), Ok(0));
        assert_eq!(watcher.used(&credential("lab")), Ok(0));
        let partitions: std::collections::BTreeSet<i32> = broker
            .records(TOPIC)
            .iter()
            .map(|record| record.partition)
            .collect();
        assert_eq!(
            partitions.into_iter().collect::<Vec<_>>(),
            [PARTITION],
            "every credential is written to one partition whatever the topic has"
        );
    }

    #[tokio::test]
    async fn gives_back_a_granted_reservation_its_caller_stopped_waiting_for() {
        let broker = MemoryBroker::new();
        let quota = quota(&broker, Duration::from_millis(50));
        quota.set_ready();
        let late = quota.reserve(reservation("lab", 1, 5)).await;
        assert!(late.unwrap_err().0.contains("not written and read back"));
        let mut read = 0;
        apply_new(&broker, &quota, &mut read);
        assert_eq!(quota.used(&credential("lab")), Ok(1));
        tokio::task::yield_now().await;
        apply_new(&broker, &quota, &mut read);
        assert_eq!(quota.used(&credential("lab")), Ok(0));

        let dropped = quota.reserve(reservation("lab", 2, 5));
        drop(dropped);
        apply_new(&broker, &quota, &mut read);
        tokio::task::yield_now().await;
        apply_new(&broker, &quota, &mut read);
        assert_eq!(quota.used(&credential("lab")), Ok(0));
        let operations: Vec<String> = broker
            .records(TOPIC)
            .iter()
            .map(|record| {
                let value: serde_json::Value =
                    serde_json::from_slice(record.payload.as_deref().unwrap()).unwrap();
                value["operation"].as_str().unwrap().to_string()
            })
            .collect();
        assert_eq!(operations, ["reserve", "release", "reserve", "release"]);
    }

    #[tokio::test]
    async fn counts_the_installations_of_an_uncapped_credential_toward_a_later_cap() {
        let broker = MemoryBroker::new();
        let quota = quota(&broker, CONFIRM_TIMEOUT);
        quota.set_ready();
        let mut read = 0;
        quota.record(&reservation(
            "lab",
            1,
            nightfall_provisioning::quota::UNCAPPED,
        ));
        tokio::task::yield_now().await;
        apply_new(&broker, &quota, &mut read);
        assert_eq!(quota.used(&credential("lab")), Ok(1));
        let late = quota.reserve(reservation("lab", 2, 1));
        let decisions = tokio::join!(late, async {
            tokio::task::yield_now().await;
            apply_new(&broker, &quota, &mut read);
        });
        assert_eq!(decisions.0, Ok(false));
        assert_eq!(quota.used(&credential("lab")), Ok(1));
    }

    struct LateProducer {
        broker: Arc<MemoryBroker>,
        delay: Duration,
        failure: Option<&'static str>,
    }

    impl RecordProducer for LateProducer {
        fn send(&self, record: OutgoingRecord) -> crate::kafka::Delivery {
            let broker = self.broker.clone();
            let delay = self.delay;
            let failure = self.failure;
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                match failure {
                    Some(reason) => Err(reason.to_string()),
                    None => broker.produce(record),
                }
            })
        }

        fn partition_count(&self, _topic: &str) -> Result<u32, String> {
            Ok(3)
        }

        fn max_message_bytes(&self) -> usize {
            crate::kafka::DEFAULT_MAX_MESSAGE_BYTES
        }

        fn flush(&self, _timeout: Duration) {}
    }

    #[tokio::test]
    async fn refuses_within_the_confirm_timeout_while_kafka_does_not_take_the_reservation() {
        let broker = MemoryBroker::new();
        broker.create_topic(TOPIC, 3, "compact");
        let late = |failure: Option<&'static str>| {
            let quota = KafkaQuota::new(
                "nightfall-0",
                TOPIC,
                Arc::new(LateProducer {
                    broker: broker.clone(),
                    delay: Duration::from_millis(300),
                    failure,
                }),
                tokio::runtime::Handle::current(),
                Duration::from_millis(50),
            );
            quota.set_ready();
            quota
        };
        let quota = late(None);
        let started = std::time::Instant::now();
        let refused = quota.reserve(reservation("lab", 1, 5)).await.unwrap_err();
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "{:?}",
            started.elapsed()
        );
        assert!(refused.0.contains("within 50 ms"), "{refused}");
        tokio::time::sleep(Duration::from_millis(400)).await;
        let mut read = 0;
        apply_new(&broker, &quota, &mut read);
        assert_eq!(quota.used(&credential("lab")), Ok(1));
        tokio::time::sleep(Duration::from_millis(400)).await;
        apply_new(&broker, &quota, &mut read);
        assert_eq!(quota.used(&credential("lab")), Ok(0));
        assert!(quota.state().abandoned.is_empty());

        let failing = late(Some("broker down"));
        assert!(failing.reserve(reservation("lab", 2, 5)).await.is_err());
        assert_eq!(failing.state().abandoned.len(), 1);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(failing.state().abandoned.is_empty());
        assert!(failing.state().waiting.is_empty());
    }

    #[tokio::test]
    async fn refuses_when_the_reservation_cannot_be_written() {
        let broker = MemoryBroker::new();
        let quota = quota(&broker, CONFIRM_TIMEOUT);
        quota.set_ready();
        broker.fail_topic(TOPIC, Some("broker down"));
        let refused = quota.reserve(reservation("lab", 1, 5)).await.unwrap_err();
        assert!(refused.0.contains("broker down"), "{refused}");
        broker.fail_topic(TOPIC, None);
        assert_eq!(quota.used(&credential("lab")), Ok(0));
    }
}
