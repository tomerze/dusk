from __future__ import annotations

import re
import uuid

import pytest
from conftest import (
    CAMPAIGN_ID,
    DEVICE_ID,
    INSTALLATION_ID,
    NAMESPACE_ID,
    PID,
    RecordingProducer,
)

from dawn.config import KafkaSettings, KafkaTopics
from dawn.events import (
    MAX_ERROR_CHARACTERS,
    MAX_MESSAGE_BYTES,
    Events,
    KafkaProducer,
    encode,
    envelope,
    fitted,
    timestamp,
)

pytestmark = pytest.mark.anyio


def test_a_timestamp_is_utc_with_nine_fractional_digits():
    assert timestamp(1791278043_000000005) == "2026-10-06T09:14:03.000000005Z"
    assert re.fullmatch(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{9}Z", timestamp())


def test_every_message_has_a_uuid_v7_id():
    message = envelope("dusk.files/v1")

    assert uuid.UUID(message["id"]).version == 7
    assert message["schema"] == "dusk.files/v1"


async def test_a_file_message_is_keyed_by_node_and_matches_its_contract(
    producer: RecordingProducer,
):
    events = Events(producer, KafkaTopics(), "dawn-1")

    await events.file(
        pid=str(PID),
        campaign_id=CAMPAIGN_ID,
        device_id=DEVICE_ID,
        installation_id=INSTALLATION_ID,
        namespace_id=NAMESPACE_ID,
        node_path="/var/log/app.log",
        bucket="dusk-files",
        object_key=f"files/{DEVICE_ID}/{INSTALLATION_ID}/{PID}/0-app.log",
        size_bytes=12,
        sha256="ab" * 32,
        content_type="application/octet-stream",
        uploaded_at=timestamp(),
    )

    ((topic, key, message),) = producer.sent
    assert topic == "dusk.files"
    assert key == f"{DEVICE_ID}/{INSTALLATION_ID}"
    assert message["dawn_instance"] == "dawn-1"


async def test_topics_come_from_the_configuration(producer: RecordingProducer):
    topics = KafkaTopics(
        process_results="tenant.process-results", process_output="o", files="f"
    )
    events = Events(producer, topics, "dawn-1")
    producer.validate = lambda topic, message: None

    delivery = await events.process_output(
        pid=str(PID),
        campaign_id=None,
        device_id=DEVICE_ID,
        installation_id=INSTALLATION_ID,
        namespace_id=NAMESPACE_ID,
        index=0,
        value=None,
        truncated=False,
    )
    await delivery

    assert producer.sent[0][0] == "o"


async def test_the_kafka_producer_is_built_with_zstd_and_idempotence():
    producer = KafkaProducer(
        KafkaSettings(brokers="kafka:9092", allow_plaintext=True), "dawn-0"
    )

    assert not producer.ready()


async def test_a_message_kafka_will_not_take_is_counted_and_logged(caplog):
    failed: list[str] = []
    producer = KafkaProducer(
        KafkaSettings(brokers="127.0.0.1:1", allow_plaintext=True),
        "dawn-0",
        failed.append,
        timeout_seconds=0.2,
    )

    delivery = await producer.send(
        "dusk.process-results", "a/b", {"id": "x", "pid": str(PID)}
    )

    assert not await delivery
    assert failed == ["dusk.process-results"]
    assert "Kafka did not take a message" in caplog.text


def test_a_message_is_encoded_as_the_utf_8_it_is_measured_in():
    assert encode({"value": "雪"}) == '{"value":"雪"}'.encode()


def test_a_message_never_carries_a_number_json_cannot_hold():
    with pytest.raises(ValueError):
        encode({"value": float("nan")})


def test_a_process_result_too_large_for_kafka_loses_its_facts_first():
    message = {
        "pid": str(PID),
        "error": "e" * (MAX_MESSAGE_BYTES + 1),
        "reported": {"version": "1", "facts": {"big": "f" * MAX_MESSAGE_BYTES}},
    }

    shortened = fitted(message)

    assert shortened["error"] == "e" * MAX_ERROR_CHARACTERS
    assert shortened["reported"] == {"version": "1", "facts": None}
    assert len(encode(shortened)) <= MAX_MESSAGE_BYTES


def test_a_process_result_that_fits_is_left_alone():
    message = {"pid": str(PID), "error": None, "reported": {"facts": {"a": 1}}}

    assert fitted(dict(message)) == message


class FlakyProducer(RecordingProducer):
    def __init__(self, refusals: int) -> None:
        super().__init__()
        self.refusals = refusals
        self.attempts = 0
        self.ids: list[str] = []

    async def send(self, topic: str, key: str, value: dict):
        self.attempts += 1
        self.ids.append(value["id"])
        if self.attempts <= self.refusals:

            async def refused() -> bool:
                return False

            return refused()
        return await super().send(topic, key, value)


async def test_a_process_result_kafka_did_not_take_is_sent_again_with_the_same_id(
    monkeypatch: pytest.MonkeyPatch,
):
    monkeypatch.setattr("dawn.events.RETRY_SECONDS", 0.001)
    producer = FlakyProducer(refusals=2)
    events = Events(producer, KafkaTopics(), "dawn-1")

    await events.process_result(
        pid=str(PID),
        campaign_id=None,
        attempt=None,
        device_id=DEVICE_ID,
        installation_id=INSTALLATION_ID,
        namespace_id=NAMESPACE_ID,
        action_kind="run_script",
        status="succeeded",
        delivered=True,
        error=None,
        started_at=timestamp(),
        finished_at=timestamp(),
        output_digest="ab" * 32,
        output_count=0,
        output_truncated=False,
        reported=None,
    )

    assert producer.attempts == 3
    assert len(producer.results()) == 1
    assert len(set(producer.ids)) == 1


async def test_dawn_is_not_ready_while_kafka_does_not_take_its_messages():
    producer = KafkaProducer(
        KafkaSettings(brokers="127.0.0.1:1", allow_plaintext=True),
        "dawn-0",
        timeout_seconds=0.2,
    )
    producer._started = True

    assert producer.ready()
    await (await producer.send("dusk.process-results", "a/b", {"id": "x"}))

    assert not producer.ready()
