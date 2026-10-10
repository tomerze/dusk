from __future__ import annotations

import json
import os
import uuid

import pytest
from aiokafka import AIOKafkaConsumer
from aiokafka.admin import AIOKafkaAdminClient, NewTopic
from conftest import (
    CAMPAIGN_ID,
    DEVICE_ID,
    INSTALLATION_ID,
    NAMESPACE_ID,
    PID,
    contract_validator,
)

from dawn.config import KafkaSettings, KafkaTopics
from dawn.events import Events, KafkaProducer

pytestmark = [pytest.mark.integration, pytest.mark.anyio]

CONSUME_MILLISECONDS = 30000


@pytest.fixture
def brokers() -> str:
    configured = os.environ.get("DAWN_KAFKA_TEST_BROKERS")
    if not configured:
        pytest.skip("set DAWN_KAFKA_TEST_BROKERS to produce to a real Kafka")
    return configured


async def test_dawn_produces_each_contract_to_a_real_kafka(brokers: str):
    prefix = f"dawn-test-{uuid.uuid4().hex[:12]}"
    topics = KafkaTopics(
        process_results=f"{prefix}.process-results",
        process_output=f"{prefix}.process-output",
        files=f"{prefix}.files",
    )
    names = {
        topics.process_results: "dusk.process-results",
        topics.process_output: "dusk.process-output",
        topics.files: "dusk.files",
    }
    administrator = AIOKafkaAdminClient(bootstrap_servers=brokers)
    await administrator.start()
    try:
        await administrator.create_topics(
            [NewTopic(name, num_partitions=3, replication_factor=1) for name in names]
        )
    finally:
        await administrator.close()

    producer = KafkaProducer(
        KafkaSettings(brokers=brokers, allow_plaintext=True, topics=topics), "dawn-0"
    )
    await producer.start()
    events = Events(producer, topics, "dawn-0")
    node = {
        "device_id": DEVICE_ID,
        "installation_id": INSTALLATION_ID,
        "namespace_id": NAMESPACE_ID,
    }
    try:
        await (
            await events.process_output(
                pid=str(PID),
                campaign_id=CAMPAIGN_ID,
                index=0,
                value={"0x84e09148e9d394f3": {"Key": ["dusk.version"]}},
                truncated=False,
                **node,
            )
        )
        await events.process_result(
            pid=str(PID),
            campaign_id=CAMPAIGN_ID,
            attempt=1,
            action_kind="run_script",
            status="succeeded",
            delivered=True,
            error=None,
            started_at="2026-10-07T10:00:00.000000000Z",
            finished_at="2026-10-07T10:00:01.000000000Z",
            output_digest="0" * 64,
            output_count=1,
            output_truncated=False,
            reported=None,
            **node,
        )
        await events.file(
            pid=str(PID),
            campaign_id=CAMPAIGN_ID,
            node_path="/var/log/syslog",
            bucket="dusk-files",
            object_key=f"files/{DEVICE_ID}/{INSTALLATION_ID}/{PID}/0-syslog",
            size_bytes=5,
            sha256="1" * 64,
            content_type="application/octet-stream",
            uploaded_at="2026-10-07T10:00:02.000000000Z",
            **node,
        )
    finally:
        await producer.stop()

    consumer = AIOKafkaConsumer(
        *names,
        bootstrap_servers=brokers,
        auto_offset_reset="earliest",
        enable_auto_commit=False,
    )
    await consumer.start()
    try:
        batches = await consumer.getmany(timeout_ms=CONSUME_MILLISECONDS)
        records = [record for batch in batches.values() for record in batch]
        while len(records) < len(names):
            more = await consumer.getmany(timeout_ms=CONSUME_MILLISECONDS)
            if not more:
                break
            records += [record for batch in more.values() for record in batch]
    finally:
        await consumer.stop()

    validate = contract_validator()
    assert sorted(record.topic for record in records) == sorted(names)
    for record in records:
        assert record.key == f"{DEVICE_ID}/{INSTALLATION_ID}".encode()
        assert record.value is not None
        message = json.loads(record.value)
        validate(names[record.topic], message)
        assert message["schema"] == f"{names[record.topic]}/v1"
