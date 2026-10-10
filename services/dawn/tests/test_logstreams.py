from __future__ import annotations

import asyncio

import pytest
from conftest import (
    DEFAULT_SH_PID,
    PID,
    FakeFleet,
    FakeNode,
    RecordingProducer,
    Wait,
    dawn_settings,
    node_ref,
)

from dawn.config import KafkaTopics
from dawn.events import Events
from dawn.logstreams import LogStreams, StreamsExhausted
from dawn.models import LogStreamRequest
from dawn.nodes import Sessions
from dawn.work import Results

pytestmark = pytest.mark.anyio


def streams(
    fleet: FakeFleet,
    producer: RecordingProducer,
    sessions: Sessions,
    max_log_streams: int = 8,
):
    counts: list[int] = []
    registry = LogStreams(
        dawn_settings(limits={"max_log_streams": max_log_streams}),
        fleet,
        sessions,
        Results(Events(producer, KafkaTopics(), "dawn-0"), b"k" * 32),
        counts.append,
    )
    return registry, counts


def log_request(**fields) -> LogStreamRequest:
    values = {
        "node": node_ref(),
        "pid": str(PID),
        "level": "info",
        "duration_seconds": 60,
        "endpoint": None,
    }
    values.update(fields)
    return LogStreamRequest(**values)


async def until(condition) -> None:
    for _ in range(500):
        if condition():
            return
        await asyncio.sleep(0.01)
    raise AssertionError("the condition never held")


async def test_a_stream_runs_the_logs_program_toward_the_collector_until_stopped(
    fleet, producer
):
    node = fleet.add(FakeNode())
    node.commands["logs"] = [Wait(60)]
    sessions = Sessions(10)
    registry, counts = streams(fleet, producer, sessions)

    stream_id = registry.start(log_request(), "operator@example.org")
    (listed,) = registry.list()
    await until(lambda: any(call[0] == "sh" for call in node.calls))
    assert await registry.stop(stream_id)

    assert listed.stream_id == stream_id
    assert listed.endpoint == "otel-collector:4317"
    assert listed.principal == "operator@example.org"
    assert listed.level == "info"
    assert listed.pid == str(PID)
    assert node.commands_in(PID) == ["logs stream otlp://otel-collector:4317 -l info"]
    (result,) = producer.results()
    assert result["action_kind"] == "stream_logs"
    assert result["status"] == "succeeded"
    assert result["delivered"] is True
    assert result["pid"] == str(PID)
    assert registry.list() == []
    assert node.calls[0] == ("connect", PID)
    assert node.commands_in(DEFAULT_SH_PID) == [
        f"kill {PID:#x}; ps",
        f"kill --signal 8 {PID:#x}; ps",
    ]
    assert PID not in node.processes
    assert sessions.open == 0
    assert counts == [1, 0]


async def test_a_stream_ends_by_itself_after_its_duration(fleet, producer):
    node = fleet.add(FakeNode())
    node.commands["logs"] = [Wait(60)]
    registry, _ = streams(fleet, producer, Sessions(10))

    registry.start(log_request(duration_seconds=1), "twilight-0")
    await until(lambda: producer.results())

    assert producer.results()[0]["status"] == "succeeded"


async def test_a_stream_goes_to_the_endpoint_asked_for(fleet, producer):
    node = fleet.add(FakeNode())
    registry, _ = streams(fleet, producer, Sessions(10))

    registry.start(
        log_request(endpoint="tenant-collector:4317", level="warn"), "twilight-0"
    )
    await until(lambda: producer.results())

    assert node.commands_in(PID) == ["logs stream otlp://tenant-collector:4317 -l warn"]


async def test_a_stream_nightfall_refuses_reports_denied(fleet, producer):
    node = fleet.add(FakeNode())
    node.commands["logs"] = [
        RuntimeError(
            "Unimplemented: remote exception: not permitted: LogsPortal.stream"
        )
    ]
    registry, _ = streams(fleet, producer, Sessions(10))

    registry.start(log_request(), "twilight-0")
    await until(lambda: producer.results())

    (result,) = producer.results()
    assert result["status"] == "denied"
    assert result["delivered"] is True


async def test_a_stream_to_an_unreachable_node_reports_it(fleet, producer):
    sessions = Sessions(10)
    registry, _ = streams(fleet, producer, sessions)

    registry.start(log_request(), "twilight-0")
    await until(lambda: producer.results())

    assert producer.results()[0]["status"] == "unreachable"
    assert producer.results()[0]["delivered"] is False
    await until(lambda: sessions.open == 0)


async def test_streams_are_bounded(fleet, producer):
    node = fleet.add(FakeNode())
    node.commands["logs"] = [Wait(60)]
    registry, _ = streams(fleet, producer, Sessions(10), max_log_streams=1)

    stream_id = registry.start(log_request(), "twilight-0")
    with pytest.raises(StreamsExhausted):
        registry.start(log_request(pid=str(PID + 1)), "twilight-0")
    await registry.stop(stream_id)


async def test_a_stream_asked_for_again_while_it_runs_is_the_same_stream(
    fleet, producer
):
    node = fleet.add(FakeNode())
    node.commands["logs"] = [Wait(60)]
    sessions = Sessions(10)
    registry, _ = streams(fleet, producer, sessions)

    stream_id = registry.start(log_request(), "twilight-0")

    assert registry.start(log_request(), "twilight-0") == stream_id
    assert sessions.open == 1
    assert registry.start(log_request(level="debug"), "twilight-0") != stream_id
    assert sessions.open == 2
    await registry.drain()


async def test_stopping_an_unknown_stream_says_so(fleet, producer):
    registry, _ = streams(fleet, producer, Sessions(10))

    assert not await registry.stop("no-such-stream")


async def test_draining_stops_every_stream_and_reports_it(fleet, producer):
    node = fleet.add(FakeNode())
    node.commands["logs"] = [Wait(60)]
    sessions = Sessions(10)
    registry, _ = streams(fleet, producer, sessions)
    registry.start(log_request(), "twilight-0")
    await until(lambda: any(call[0] == "sh" for call in node.calls))

    await registry.drain()

    (result,) = producer.results()
    assert result["status"] == "error"
    assert result["error"] == "dawn stopped the log stream"
    assert sessions.open == 0
    assert registry.list() == []
    assert PID not in node.processes
