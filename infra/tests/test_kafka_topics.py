from typing import Any

import pytest
from confluent_kafka.admin import AdminClient
from confluent_kafka.admin._config import ConfigResource
from conftest import required

DAY = 86400000
TOPICS: dict[str, tuple[int, dict[str, str]]] = {
    "dusk.connections": (3, {"cleanup.policy": "delete", "retention.ms": str(7 * DAY)}),
    "dusk.census": (
        1,
        {
            "cleanup.policy": "compact",
            "max.message.bytes": "2097152",
            "segment.ms": "600000",
            "min.cleanable.dirty.ratio": "0.1",
            "delete.retention.ms": "3600000",
        },
    ),
    "dusk.ledger": (
        3,
        {
            "cleanup.policy": "delete",
            "retention.ms": str(30 * DAY),
            "unclean.leader.election.enable": "false",
        },
    ),
    "dusk.enrollments": (
        3,
        {"cleanup.policy": "delete", "retention.ms": str(30 * DAY)},
    ),
    "dusk.node-state": (1, {"cleanup.policy": "compact"}),
    "dusk.process-results": (
        3,
        {"cleanup.policy": "delete", "retention.ms": str(7 * DAY)},
    ),
    "dusk.process-output": (
        3,
        {"cleanup.policy": "delete", "retention.ms": str(7 * DAY)},
    ),
    "dusk.files": (3, {"cleanup.policy": "delete", "retention.ms": str(30 * DAY)}),
    "dusk.intended-processes": (
        3,
        {
            "cleanup.policy": "compact",
            "segment.ms": "600000",
            "min.cleanable.dirty.ratio": "0.1",
            "delete.retention.ms": "3600000",
        },
    ),
    "dusk.otel-logs": (
        3,
        {
            "cleanup.policy": "delete",
            "retention.ms": str(3 * DAY),
            "max.message.bytes": "4194304",
        },
    ),
    "dusk.otel-spans": (
        3,
        {
            "cleanup.policy": "delete",
            "retention.ms": str(3 * DAY),
            "max.message.bytes": "4194304",
        },
    ),
    "dusk.otel-metrics": (
        3,
        {
            "cleanup.policy": "delete",
            "retention.ms": str(3 * DAY),
            "max.message.bytes": "4194304",
        },
    ),
}


@pytest.fixture(scope="module")
def admin() -> AdminClient:
    return AdminClient(
        {
            "bootstrap.servers": required("KAFKA_BROKERS"),
            "allow.auto.create.topics": "false",
        }
    )


def test_exactly_the_contract_topics_exist(admin: AdminClient) -> None:
    topics = {
        name
        for name in admin.list_topics(timeout=10).topics
        if not name.startswith("__")
    }
    assert topics == set(TOPICS)


@pytest.mark.parametrize("topic", sorted(TOPICS))
def test_topic_has_the_dev_partitions_and_its_contract_configuration(
    admin: AdminClient, topic: str
) -> None:
    partitions, expected = TOPICS[topic]
    assert (
        len(admin.list_topics(topic=topic, timeout=10).topics[topic].partitions)
        == partitions
    )
    resource = ConfigResource(ConfigResource.Type.TOPIC, topic)
    entries: dict[str, Any] = admin.describe_configs([resource])[resource].result(
        timeout=10
    )
    actual = {name: entries[name].value for name in expected}
    assert actual == expected
    assert entries["min.insync.replicas"].value == "1"


def test_brokers_never_create_topics(admin: AdminClient) -> None:
    broker = next(iter(admin.list_topics(timeout=10).brokers))
    resource = ConfigResource(ConfigResource.Type.BROKER, str(broker))
    entries = admin.describe_configs([resource])[resource].result(timeout=10)
    assert entries["auto.create.topics.enable"].value == "false"
