use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use nightfall_ledger::entry::{Direction, EntryContent, Event, Kind, NIGHTFALL_PRINCIPAL};
use nightfall_ledger::kafka::{KafkaLog, KafkaLogConfig, PartitionRange, verify_partition};
use nightfall_ledger::log::LedgerLog;
use nightfall_ledger::signing::{CheckpointSigner, VerifyingKeys};
use nightfall_ledger::verifier::{VerificationReport, VerifyOptions};
use nightfall_ledger::writer::{
    CommitFailure, LedgerConfig, LedgerStatus, LedgerThread, LedgerWriter, NoObserver,
};
use rdkafka::ClientConfig;
use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use ring::rand::SystemRandom;
use ring::signature::Ed25519KeyPair;

const DEVICE: &str = "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13";
const INSTALLATION: &str = "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70";
const SESSION: &str = "0192f3a4-9e8d-7c6b-8a59-483726150f1e";

struct Broker {
    brokers: String,
    topic: String,
    pkcs8: Vec<u8>,
}

impl Broker {
    fn log_config(&self, partition: u32, instance: &str) -> KafkaLogConfig {
        KafkaLogConfig {
            brokers: self.brokers.clone(),
            properties: BTreeMap::new(),
            topic: self.topic.clone(),
            partition,
            instance: String::from(instance),
            operation_timeout: Duration::from_secs(30),
        }
    }

    fn signer(&self) -> CheckpointSigner {
        CheckpointSigner::from_pkcs8_der(&self.pkcs8).unwrap()
    }

    fn start(&self, instance: &str) -> (LedgerWriter, LedgerThread) {
        let log = KafkaLog::connect(self.log_config(1, instance)).unwrap();
        let config = LedgerConfig {
            checkpoint_interval: Duration::from_secs(3600),
            ..LedgerConfig::new(instance, 1)
        };
        LedgerWriter::start(config, Box::new(log), self.signer(), Arc::new(NoObserver)).unwrap()
    }

    async fn verify(self: &Arc<Self>) -> VerificationReport {
        let broker = self.clone();
        tokio::task::spawn_blocking(move || {
            let keys =
                VerifyingKeys::from_jwks(&broker.signer().public_jwks().to_string()).unwrap();
            let range = PartitionRange {
                topic: broker.topic.clone(),
                partition: 1,
                start: None,
                end: None,
            };
            verify_partition(
                &broker.brokers,
                &BTreeMap::new(),
                &range,
                keys,
                VerifyOptions::default(),
                Duration::from_secs(30),
            )
            .unwrap()
        })
        .await
        .unwrap()
    }
}

async fn stop(writer: &LedgerWriter, thread: LedgerThread) {
    writer.shutdown();
    tokio::task::spawn_blocking(move || thread.join())
        .await
        .unwrap();
}

fn session_event(event: Event) -> EntryContent {
    EntryContent {
        device_id: Some(String::from(DEVICE)),
        installation_id: Some(String::from(INSTALLATION)),
        namespace_id: Some(0x5d2e9a1c7b3f8e04),
        epoch: Some(1791278043512408),
        ..EntryContent::event(event, NIGHTFALL_PRINCIPAL)
    }
}

fn call(call_id: &str) -> EntryContent {
    EntryContent {
        kind: Kind::Call,
        session_id: Some(String::from(SESSION)),
        call_id: Some(String::from(call_id)),
        cap_id: Some(1),
        parent_cap_id: Some(0),
        direction: Some(Direction::ClientToNode),
        action: Some(String::from("Dusk.ps")),
        interface_id: Some(0xace6963097d486d6),
        method_id: Some(2),
        param_hash: "1f".repeat(32),
        principal: String::from("dawn-0"),
        event: None,
        ..session_event(Event::SessionOpen)
    }
}

async fn create_topic(brokers: &str, topic: &str, partitions: i32) {
    let admin: AdminClient<DefaultClientContext> = ClientConfig::new()
        .set("bootstrap.servers", brokers)
        .create()
        .unwrap();
    let created = admin
        .create_topics(
            &[NewTopic::new(topic, partitions, TopicReplication::Fixed(1))],
            &AdminOptions::new().operation_timeout(Some(Duration::from_secs(30))),
        )
        .await
        .unwrap();
    for outcome in created {
        outcome.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a Kafka broker in NIGHTFALL_KAFKA_TEST_BROKERS"]
async fn writes_continues_fences_links_and_verifies_a_chain_on_a_real_broker() {
    let brokers = std::env::var("NIGHTFALL_KAFKA_TEST_BROKERS")
        .ok()
        .filter(|brokers| !brokers.is_empty())
        .expect("NIGHTFALL_KAFKA_TEST_BROKERS names the Kafka bootstrap servers to test against");
    let topic = format!("nightfall-ledger-test-{}", uuid::Uuid::now_v7());
    create_topic(&brokers, &topic, 2).await;
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .unwrap()
        .as_ref()
        .to_vec();
    let broker = Arc::new(Broker {
        brokers,
        topic,
        pkcs8,
    });

    let beyond = KafkaLog::connect(broker.log_config(2, "nightfall-2"));
    let refusal = beyond.err().unwrap().message;
    assert!(refusal.contains("has 2 partitions"), "{refusal}");

    let (writer, thread) = broker.start("nightfall-1");
    let mut reservation = writer.reserve(3).unwrap();
    reservation
        .record(session_event(Event::SessionOpen))
        .unwrap();
    reservation
        .record_write_ahead(call("0192f3a4-a001-7b2c-9d3e-4f5061728394"))
        .unwrap()
        .await
        .unwrap();
    reservation
        .record(session_event(Event::SessionClose))
        .unwrap();
    stop(&writer, thread).await;

    let (writer, thread) = broker.start("nightfall-1");
    let mut reservation = writer.reserve(2).unwrap();
    reservation
        .record_write_ahead(session_event(Event::SessionOpen))
        .unwrap()
        .await
        .unwrap();
    let mut fencing = KafkaLog::connect(broker.log_config(1, "nightfall-1")).unwrap();
    let commit = reservation
        .record_write_ahead(call("0192f3a4-a002-7b2c-9d3e-4f5061728394"))
        .unwrap();
    assert_eq!(commit.await, Err(CommitFailure::Fenced));
    tokio::task::spawn_blocking(move || thread.join())
        .await
        .unwrap();
    assert_eq!(writer.status(), LedgerStatus::Fenced);
    assert_eq!(fencing.read_tail(1).unwrap().len(), 1);
    drop(fencing);

    let report = broker.verify().await;
    assert!(report.is_clean(), "{report}");
    assert_eq!(report.entries, 5);
    assert_eq!(report.chains.len(), 1);
    assert_eq!(report.chains[0].last_sequence, 4);
    assert_eq!(report.chains[0].last_checkpoint_sequence, Some(3));

    let (writer, thread) = broker.start("nightfall-1-replacement");
    stop(&writer, thread).await;
    let report = broker.verify().await;
    assert!(report.is_clean(), "{report}");
    assert_eq!(report.entries, 7);
    assert_eq!(report.chains.len(), 2);
    assert_eq!(report.chains[1].instance, "nightfall-1-replacement");
    assert_eq!(report.chains[1].last_checkpoint_sequence, Some(1));
}
