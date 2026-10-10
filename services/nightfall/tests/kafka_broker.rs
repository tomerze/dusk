#[allow(dead_code)]
mod support;

use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS};
use nightfall::config::Topics;
use nightfall::contracts::Contract;
use nightfall::kafka::{Broker, KafkaBroker, Start};
use nightfall::server::Services;
use nightfall_ledger::kafka::{KafkaLog, KafkaLogConfig, PartitionRange, verify_partition};
use nightfall_ledger::signing::VerifyingKeys;
use nightfall_ledger::verifier::VerifyOptions;
use rdkafka::ClientConfig;
use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use support::harness::{
    DEVICE, Environment, INSTALLATION, connect_client, free_port, intended_process_record,
    link_node, local_session, node_state_record, run, shutdown, wait_for_session, wait_ready,
    wait_until,
};
use support::node::{ps, run_script, shell_server_request};

fn brokers() -> String {
    std::env::var("NIGHTFALL_KAFKA_TEST_BROKERS")
        .expect("NIGHTFALL_KAFKA_TEST_BROKERS names the Kafka bootstrap servers to test against")
}

fn create_topics(brokers: &str, suffix: &str) -> Topics {
    let topics = Topics {
        connections: format!("dusk.connections.{suffix}"),
        census: format!("dusk.census.{suffix}"),
        ledger: format!("dusk.ledger.{suffix}"),
        enrollments: format!("dusk.enrollments.{suffix}"),
        node_state: format!("dusk.node-state.{suffix}"),
        intended_processes: format!("dusk.intended-processes.{suffix}"),
    };
    let admin: AdminClient<DefaultClientContext> = ClientConfig::new()
        .set("bootstrap.servers", brokers)
        .create()
        .unwrap();
    let new_topics = [
        NewTopic::new(&topics.connections, 3, TopicReplication::Fixed(1))
            .set("cleanup.policy", "delete"),
        NewTopic::new(&topics.census, 1, TopicReplication::Fixed(1))
            .set("cleanup.policy", "compact"),
        NewTopic::new(&topics.ledger, 2, TopicReplication::Fixed(1))
            .set("cleanup.policy", "delete"),
        NewTopic::new(&topics.enrollments, 3, TopicReplication::Fixed(1))
            .set("cleanup.policy", "delete"),
        NewTopic::new(&topics.node_state, 1, TopicReplication::Fixed(1))
            .set("cleanup.policy", "compact"),
        NewTopic::new(&topics.intended_processes, 3, TopicReplication::Fixed(1))
            .set("cleanup.policy", "compact"),
    ];
    let created = futures::executor::block_on(admin.create_topics(
        new_topics.iter(),
        &AdminOptions::new().operation_timeout(Some(Duration::from_secs(30))),
    ))
    .unwrap();
    for outcome in created {
        outcome.unwrap();
    }
    topics
}

fn read_all(broker: &KafkaBroker, topic: &str) -> Vec<Value> {
    let mut consumer = broker.consumer("test", topic, Start::Beginning).unwrap();
    let targets = consumer.watermarks().unwrap();
    let mut values = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !nightfall::kafka::caught_up(&targets, &consumer.positions().unwrap()) {
        assert!(
            std::time::Instant::now() < deadline,
            "{topic} was not read to its end"
        );
        if let Some(record) = consumer.poll(Duration::from_millis(200)).unwrap()
            && let Some(payload) = record.payload
        {
            values.push(serde_json::from_slice(&payload).unwrap());
        }
    }
    values
}

#[test]
#[ignore = "needs a Kafka broker: set NIGHTFALL_KAFKA_TEST_BROKERS"]
fn nightfall_runs_its_sessions_census_ledger_and_node_state_through_a_real_broker() {
    let brokers = brokers();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let topics = create_topics(&brokers, &suffix);
    let environment = Environment::new();
    let instance_name = format!("nightfall-{suffix}-0");
    let mut config = environment.config(&instance_name, 0);
    config.kafka.brokers = brokers.clone();
    config.kafka.topics = topics.clone();
    let broker = KafkaBroker::new(&brokers, &BTreeMap::new(), &instance_name);
    let ledger_log = KafkaLog::connect(KafkaLogConfig {
        brokers: brokers.clone(),
        properties: BTreeMap::new(),
        topic: topics.ledger.clone(),
        partition: 0,
        instance: instance_name.clone(),
        operation_timeout: Duration::from_secs(30),
    })
    .unwrap();
    let instance = nightfall::server::start(
        config,
        Services {
            broker: Arc::new(KafkaBroker::new(&brokers, &BTreeMap::new(), &instance_name)),
            ledger_log: Box::new(ledger_log),
        },
    )
    .unwrap();
    wait_ready(&instance);
    let node_port = free_port();
    let node = DuskNixImpl::new(LISTEN_ADDRESS, node_port);
    node.expect_errors();
    let producer = broker.producer("test").unwrap();
    run(async {
        let _bridge = link_node(
            instance.addresses.fleet,
            node_port,
            environment.node_identity(DEVICE, INSTALLATION),
        )
        .await;
        let (namespace_id, _, _) = wait_for_session(&instance).await;
        let client = connect_client(
            instance.addresses.inner,
            namespace_id,
            environment.principal("operator-0"),
        )
        .await;
        assert!(ps(&client.dusk).await.unwrap() > 0);
        let intended = 0x5eed_0000_0000_0000 | u64::from(node_port);
        let dawn = connect_client(
            instance.addresses.inner,
            namespace_id,
            environment.principal("dawn-0"),
        )
        .await;
        producer
            .send(intended_process_record(
                &topics.intended_processes,
                INSTALLATION,
                intended,
                1,
                0,
            ))
            .await
            .unwrap();
        assert_eq!(
            run_script(&dawn.dusk, Some(intended), "echo intended").await,
            ["intended"]
        );
        let refused = shell_server_request(&dawn.dusk, Some(intended + 1))
            .promise
            .await
            .err()
            .unwrap();
        assert!(refused.extra.contains("denied: not intended"), "{refused}");
        producer
            .send(node_state_record(
                &topics.node_state,
                DEVICE,
                None,
                Some("revoked"),
            ))
            .await
            .unwrap();
        wait_until(
            "the revoked session to close",
            Duration::from_secs(20),
            || local_session(&instance).is_none(),
        )
        .await;
    });
    shutdown(instance);

    let connections = read_all(&broker, &topics.connections);
    let validator = Contract::Connections.validator().unwrap();
    for message in &connections {
        validator.check(message.to_string().as_bytes()).unwrap();
    }
    assert_eq!(connections.len(), 2, "{connections:?}");
    assert_eq!(connections[1]["disconnect_reason"], "revoked");
    let census = read_all(&broker, &topics.census);
    let validator = Contract::Census.validator().unwrap();
    for message in &census {
        validator.check(message.to_string().as_bytes()).unwrap();
    }
    assert!(census.iter().any(|message| message["record"] == "header"));
    let report = verify_partition(
        &brokers,
        &BTreeMap::new(),
        &PartitionRange {
            topic: topics.ledger.clone(),
            partition: 0,
            start: None,
            end: None,
        },
        VerifyingKeys::from_jwks(&environment.ledger_key.public_jwks()).unwrap(),
        VerifyOptions::default(),
        Duration::from_secs(30),
    )
    .unwrap();
    assert!(report.is_clean(), "{report}");
    assert!(report.entries > 5, "{report}");
    assert!(report.checkpoints > 0, "{report}");
}
