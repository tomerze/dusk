from __future__ import annotations

import asyncio
import hashlib
import hmac
import json
import time

import pytest
from conftest import (
    CAMPAIGN_ID,
    DEFAULT_SH_PID,
    DEVICE_ID,
    INSTALLATION_ID,
    NAMESPACE_ID,
    PID,
    FakeFleet,
    FakeNode,
    RecordingProducer,
    Wait,
    dawn_settings,
    node_ref,
    script_span,
    work_spec,
)

from dawn.config import KafkaTopics
from dawn.events import Events
from dawn.nodes import Unreachable
from dawn.work import Results, Runner, classify, ordered

pytestmark = pytest.mark.anyio

OUTPUT_KEY = b"output-digest-key-of-32-bytes!!!"
OTHER_PID = 0x7E570000


def runner(
    producer: RecordingProducer, fleet: FakeFleet, max_output_bytes: int = 1048576
) -> Runner:
    return Runner(
        dawn_settings(limits={"max_output_bytes": max_output_bytes}),
        fleet,
        Results(Events(producer, KafkaTopics(), "dawn-0"), OUTPUT_KEY),
    )


def digest_of(*values) -> str:
    digest = hmac.new(OUTPUT_KEY, digestmod=hashlib.sha256)
    for value in values:
        encoded = json.dumps(
            value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
        )
        digest.update(encoded.encode() + b"\n")
    return digest.hexdigest()


@pytest.fixture
def node(fleet: FakeFleet) -> FakeNode:
    return fleet.add(FakeNode())


async def run(producer, fleet, **work_fields) -> str:
    return await runner(producer, fleet).run(node_ref(), work_spec(**work_fields))


async def test_work_runs_once_in_a_shell_server_at_its_pid(producer, fleet, node):
    assert await run(producer, fleet) == "succeeded"

    assert node.calls == [
        ("connect", DEFAULT_SH_PID),
        ("sh", DEFAULT_SH_PID, "ps"),
        ("connect", PID),
        ("sh", PID, "echo hello"),
        ("sh", DEFAULT_SH_PID, f"kill {PID:#x}; ps"),
        ("disconnect", DEFAULT_SH_PID),
        ("disconnect", PID),
    ]
    assert node.processes[PID] == ["sh[server]", "Z"]
    assert node.processes[DEFAULT_SH_PID] == ["sh[server]", "RR"]


async def test_work_that_succeeds_reports_started_then_succeeded(producer, fleet, node):
    await run(producer, fleet)

    started, finished = producer.results()
    assert started["status"] == "started"
    assert started["delivered"] is True
    assert started["finished_at"] is None
    assert finished["status"] == "succeeded"
    assert finished["pid"] == str(PID)
    assert finished["attempt"] == 1
    assert finished["delivered"] is True
    assert finished["error"] is None
    assert finished["started_at"] == started["started_at"]
    assert finished["finished_at"] is not None
    assert finished["output_count"] == 1
    assert finished["output_truncated"] is False
    assert finished["output_digest"] == digest_of("hello")
    assert finished["campaign_id"] == CAMPAIGN_ID
    assert finished["namespace_id"] == NAMESPACE_ID
    (output,) = producer.on("dusk.process-output")
    assert output["pid"] == str(PID)
    assert output["value"] == "hello"
    assert output["index"] == 0
    assert output["truncated"] is False
    assert producer.sent[-1][1] == f"{DEVICE_ID}/{INSTALLATION_ID}"


async def test_work_whose_script_fails_carries_the_nodes_error(producer, fleet, node):
    node.scripts["false"] = [RuntimeError("Failed: program exited with error")]

    assert await run(producer, fleet, script="false") == "failed"

    finished = producer.results()[-1]
    assert finished["delivered"] is True
    assert finished["error"] == "Failed: program exited with error"


async def test_a_script_failing_with_a_permission_error_failed_and_was_not_denied(
    producer, fleet, node
):
    node.scripts["cat /root/secret"] = [
        "Permission denied (os error 13)",
        RuntimeError(
            "Failed: remote exception: remote exception: couldn't open `/root/secret`: "
            "Permission denied (os error 13)"
        ),
    ]

    assert await run(producer, fleet, script="cat /root/secret") == "failed"

    finished = producer.results()[-1]
    assert finished["delivered"] is True
    assert finished["error"].endswith("Permission denied (os error 13)")


async def test_a_pid_running_on_the_node_is_a_duplicate_and_nothing_runs(
    producer, fleet, node
):
    node.processes[PID] = ["sh[server]", "RR"]

    assert await run(producer, fleet) == "duplicate"

    assert node.commands_in(PID) == []
    assert ("connect", PID) not in node.calls
    (finished,) = producer.results()
    assert finished["delivered"] is False
    assert finished["reported"] is None


async def test_a_pid_that_exited_unreaped_is_still_a_duplicate(producer, fleet, node):
    node.processes[PID] = ["sh[server]", "Z"]

    assert await run(producer, fleet) == "duplicate"

    assert node.processes[PID] == ["sh[server]", "Z"]


async def test_work_resent_after_it_ran_is_a_duplicate(producer, fleet, node):
    work = runner(producer, fleet)
    await work.run(node_ref(), work_spec())

    assert await work.run(node_ref(), work_spec()) == "duplicate"

    assert node.commands_in(PID) == ["echo hello"]
    assert [result["status"] for result in producer.results()] == [
        "started",
        "succeeded",
        "duplicate",
    ]


async def test_work_nightfall_refuses_is_denied_and_never_delivered(
    producer, fleet, node
):
    node.refusal = RuntimeError(
        "Unimplemented: remote exception: not permitted: Dusk.process"
    )

    assert await run(producer, fleet) == "denied"

    (finished,) = producer.results()
    assert finished["delivered"] is False
    assert "Unimplemented" in finished["error"]


async def test_work_for_a_node_nightfall_does_not_hold_is_unreachable(producer, fleet):
    assert await run(producer, fleet) == "unreachable"

    (finished,) = producer.results()
    assert finished["delivered"] is False


async def test_a_stream_that_breaks_while_the_logs_show_no_end_is_running(
    producer, fleet, node
):
    node.scripts["echo hello"] = ["hello", RuntimeError("Disconnected: peer hung up")]
    node.log_records = [script_span(PID, ended=False), script_span(OTHER_PID)]

    assert await run(producer, fleet, collect_facts=True) == "running"

    finished = producer.results()[-1]
    assert finished["delivered"] is True
    assert finished["output_count"] == 1
    assert finished["error"] == "Disconnected: peer hung up"
    assert finished["reported"] is None
    assert node.commands_in(DEFAULT_SH_PID) == [
        "ps",
        "logs dump --replay-only -l info",
    ]
    assert node.commands_in(PID) == ["echo hello"]
    assert node.processes[PID] == ["sh[server]", "RR"]


async def test_a_stream_that_breaks_after_the_logs_show_the_end_has_ended(
    producer, fleet, node
):
    node.scripts["echo hello"] = [RuntimeError("Disconnected: peer hung up")]
    node.log_records = ["a line", script_span(PID)]

    assert await run(producer, fleet, collect_facts=True) == "ended"

    finished = producer.results()[-1]
    assert finished["delivered"] is True
    assert finished["error"] == "Disconnected: peer hung up"
    assert finished["reported"] is None
    assert node.commands_in(DEFAULT_SH_PID)[-1] == f"kill {PID:#x}; ps"
    assert node.processes[PID] == ["sh[server]", "Z"]


async def test_a_stream_that_breaks_when_the_logs_cannot_be_read_is_running(
    producer, fleet, node
):
    node.scripts["echo hello"] = [RuntimeError("Connection is closed")]
    node.scripts["logs dump --replay-only -l info"] = [
        RuntimeError("Disconnected: gone")
    ]

    assert await run(producer, fleet) == "running"


async def test_work_that_times_out_before_delivery_was_not_delivered(
    producer, fleet, node
):
    node.commands["ps"] = [Wait(5)]

    assert await run(producer, fleet, timeout_seconds=1) == "timed_out"

    (finished,) = producer.results()
    assert finished["delivered"] is False


async def test_work_that_times_out_after_delivery_was_delivered_and_runs_once(
    producer, fleet, node
):
    work = runner(producer, fleet)
    node.scripts["echo hello"] = [Wait(5)]

    assert await work.run(node_ref(), work_spec(timeout_seconds=1)) == "timed_out"
    assert producer.results()[-1]["delivered"] is True

    assert await work.run(node_ref(), work_spec(timeout_seconds=1)) == "duplicate"
    assert node.commands_in(PID) == ["echo hello"]


async def test_a_connection_at_the_pid_that_comes_up_late_leaves_no_pid_behind(
    producer, fleet, node
):
    def slow_at_the_pid(target, sh_server_pid):
        if sh_server_pid is not None:
            time.sleep(0.3)
        return fleet(target, sh_server_pid)

    settings = dawn_settings(nightfall={"connect_timeout_seconds": 0.1})
    results = Results(Events(producer, KafkaTopics(), "dawn-0"), OUTPUT_KEY)

    status = await Runner(settings, slow_at_the_pid, results).run(
        node_ref(), work_spec()
    )

    assert status == "unreachable"
    (finished,) = producer.results()
    assert finished["delivered"] is False
    assert node.commands_in(PID) == []
    assert PID not in node.processes
    assert node.commands_in(DEFAULT_SH_PID)[-2:] == [
        f"kill {PID:#x}; ps",
        f"kill --signal 8 {PID:#x}; ps",
    ]


async def test_a_pid_a_failed_connection_left_that_stays_is_an_error_delivered(
    producer, fleet, node, monkeypatch: pytest.MonkeyPatch
):
    monkeypatch.setattr("dawn.work.REAP_WAIT_SECONDS", 0.001)
    node.scripts[f"kill {PID:#x}; ps"] = lambda connection: node.ps()

    def refused_after_registering(target, sh_server_pid):
        if sh_server_pid is not None:
            node.processes[sh_server_pid] = ["sh[server]", "RR"]
            raise RuntimeError("Disconnected: couldn't attach to the shell server")
        return fleet(target, sh_server_pid)

    results = Results(Events(producer, KafkaTopics(), "dawn-0"), OUTPUT_KEY)
    status = await Runner(dawn_settings(), refused_after_registering, results).run(
        node_ref(), work_spec()
    )

    assert status == "error"
    (finished,) = producer.results()
    assert finished["delivered"] is True
    assert finished["error"].startswith(
        "a shell server stayed at the pid after its connection failed: "
    )


async def test_output_kafka_did_not_take_marks_the_output_truncated(fleet, node):
    class Losing(RecordingProducer):
        async def send(self, topic: str, key: str, value: dict):
            delivery = await super().send(topic, key, value)
            if topic != "dusk.process-output":
                return delivery
            delivery.close()

            async def lost() -> bool:
                return False

            return lost()

    producer = Losing()
    node.scripts["echo hello"] = ["one", "two"]

    assert await run(producer, fleet) == "succeeded"

    finished = producer.results()[-1]
    assert finished["output_truncated"] is True
    assert finished["output_count"] == 2


async def test_output_beyond_the_cap_is_counted_and_digested_but_not_produced(
    producer, fleet, node
):
    node.scripts["echo hello"] = ["a" * 40, "b" * 40, "c" * 40]

    await runner(producer, fleet, max_output_bytes=100).run(node_ref(), work_spec())

    outputs = producer.on("dusk.process-output")
    assert [output["value"] for output in outputs] == ["a" * 40, "b" * 40, None]
    assert outputs[-1]["truncated"] is True
    finished = producer.results()[-1]
    assert finished["output_count"] == 3
    assert finished["output_truncated"] is True
    assert finished["output_digest"] == digest_of("a" * 40, "b" * 40, "c" * 40)


async def test_a_value_json_cannot_carry_is_rendered_as_dusk_gw_renders_it(
    producer, fleet, node
):
    node.scripts["echo hello"] = [b"\x00\x01", float("nan")]

    await run(producer, fleet)

    assert [output["value"] for output in producer.on("dusk.process-output")] == [
        "b'\\x00\\x01'",
        "nan",
    ]


async def test_ensure_version_already_at_the_desired_version_runs_nothing(
    producer, fleet, node
):
    status = await run(
        producer,
        fleet,
        kind="ensure_version",
        version_key="dusk.version",
        desired_version="0.1.0",
    )

    assert status == "already_satisfied"
    assert ("connect", PID) not in node.calls
    assert node.commands_in(DEFAULT_SH_PID) == [
        "ps",
        "kvs get dusk.version",
        "kvs get dusk.config.hash",
    ]
    (finished,) = producer.results()
    assert finished["delivered"] is False
    assert finished["reported"]["version_key"] == "dusk.version"
    assert finished["reported"]["version"] == "0.1.0"
    assert finished["reported"]["services"] == ["nightfall", "sh[server]"]


async def test_ensure_version_runs_the_script_and_reads_the_version_after(
    producer, fleet, node
):
    def update(connection):
        node.kvs["dusk.version"] = "0.2.0"
        return ["updated"]

    node.scripts["update"] = update

    status = await run(
        producer,
        fleet,
        kind="ensure_version",
        script="update",
        version_key="dusk.version",
        desired_version="0.2.0",
    )

    assert status == "succeeded"
    assert node.commands_in(PID) == ["update"]
    assert node.commands_in(DEFAULT_SH_PID)[3:] == [
        "ps",
        "kvs get dusk.version",
        "kvs get dusk.config.hash",
        f"kill {PID:#x}; ps",
    ]
    assert producer.results()[-1]["reported"]["version"] == "0.2.0"


async def test_ensure_version_with_the_key_absent_runs_the_script(
    producer, fleet, node
):
    status = await run(
        producer,
        fleet,
        kind="ensure_version",
        version_key="app.version",
        desired_version="3.1.0",
    )

    assert status == "succeeded"
    assert node.commands_in(PID)[0] == "echo hello"
    assert producer.results()[-1]["reported"]["version"] is None


async def test_ensure_config_reads_the_config_hash_the_node_never_registered(
    producer, fleet, node
):
    node.kvs["dusk.config.hash"] = "ab" * 32

    status = await run(producer, fleet, kind="ensure_config", config_hash="ab" * 32)

    assert status == "already_satisfied"
    assert producer.results()[0]["reported"]["config_hash"] == "ab" * 32


async def test_ensure_config_reports_the_config_hash_after(producer, fleet, node):
    def configure(connection):
        node.kvs["dusk.config.hash"] = "cd" * 32
        return []

    node.scripts["kvs set dusk.config.hash x"] = configure

    status = await run(
        producer,
        fleet,
        kind="ensure_config",
        script="kvs set dusk.config.hash x",
        config_hash="cd" * 32,
    )

    assert status == "succeeded"
    assert producer.results()[-1]["reported"]["config_hash"] == "cd" * 32


async def test_a_duplicate_ensure_reports_the_state_read_in_the_default_shell(
    producer, fleet, node
):
    node.processes[PID] = ["sh[server]", "Z"]

    status = await run(
        producer,
        fleet,
        kind="ensure_version",
        version_key="dusk.version",
        desired_version="0.2.0",
    )

    assert status == "duplicate"
    (finished,) = producer.results()
    assert finished["delivered"] is False
    assert finished["reported"]["version"] == "0.1.0"
    assert node.commands_in(DEFAULT_SH_PID)[1:] == [
        "kvs get dusk.version",
        "kvs get dusk.config.hash",
    ]


async def test_a_state_that_cannot_be_read_after_the_work_keeps_its_status(
    producer, fleet, node
):
    def update(connection):
        node.commands["kvs"] = [RuntimeError("Disconnected: gone")]
        node.commands["ps"] = [RuntimeError("Disconnected: gone")]
        return []

    node.scripts["update"] = update

    status = await run(
        producer,
        fleet,
        kind="ensure_version",
        script="update",
        version_key="dusk.version",
        desired_version="0.2.0",
    )

    assert status == "succeeded"
    assert producer.results()[-1]["reported"]["version"] == "0.1.0"


async def test_collected_facts_never_carry_the_device_id(producer, fleet, node):
    await run(producer, fleet, collect_facts=True)

    reported = producer.results()[-1]["reported"]
    assert "dusk.device.id" not in reported["facts"]
    assert reported["facts"]["dusk.hostname"] == "node-1"
    assert node.commands_in(DEFAULT_SH_PID)[1] == "kvs get dusk.; ps"


async def test_work_streams_the_nodes_logs_to_the_collector_in_its_shell(
    producer, fleet, node
):
    node.commands["logs"] = [Wait(5)]

    await run(
        producer,
        fleet,
        stream_logs={"level": "debug", "duration_seconds": 1},
    )

    assert node.commands_in(PID) == [
        "echo hello",
        "logs stream otlp://otel-collector:4317 -l debug",
    ]
    assert producer.results()[-1]["status"] == "succeeded"


async def test_the_shell_is_stopped_even_when_the_logs_stream_until_the_deadline(
    producer, fleet, node
):
    node.commands["logs"] = [Wait(30)]

    status = await run(
        producer,
        fleet,
        timeout_seconds=1,
        stream_logs={"level": "info", "duration_seconds": 600},
    )

    assert status == "succeeded"
    assert node.commands_in(DEFAULT_SH_PID)[-1] == f"kill {PID:#x}; ps"
    assert node.processes[PID] == ["sh[server]", "Z"]


async def test_nothing_runs_after_denied_work(producer, fleet, node):
    node.scripts["echo hello"] = [
        RuntimeError("Unimplemented: remote exception: not permitted: ShPortal.sh")
    ]

    status = await run(
        producer,
        fleet,
        collect_facts=True,
        stream_logs={"level": "info", "duration_seconds": 1},
    )

    assert status == "denied"
    assert node.commands_in(PID) == ["echo hello"]


async def test_work_dawn_stops_while_its_script_runs_is_running_and_left_alone(
    producer, fleet, node
):
    node.scripts["echo hello"] = ["first", Wait(30)]
    node.log_records = [script_span(PID, ended=False)]
    running = asyncio.create_task(runner(producer, fleet).run(node_ref(), work_spec()))
    await until(lambda: len(producer.on("dusk.process-output")) == 1)

    running.cancel()
    with pytest.raises(asyncio.CancelledError):
        await running

    finished = producer.results()[-1]
    assert finished["status"] == "running"
    assert finished["error"] == "dawn stopped before the script ended"
    assert finished["delivered"] is True
    assert finished["output_count"] == 1
    assert node.commands_in(DEFAULT_SH_PID) == ["ps", "logs dump --replay-only -l info"]
    assert node.processes[PID] == ["sh[server]", "RR"]
    assert all(connection.disconnected for connection in node.connections)


async def test_work_dawn_stops_after_the_logs_show_its_end_has_ended_and_is_stopped(
    producer, fleet, node
):
    node.scripts["echo hello"] = ["first", Wait(30)]
    node.log_records = [script_span(PID)]
    running = asyncio.create_task(runner(producer, fleet).run(node_ref(), work_spec()))
    await until(lambda: len(producer.on("dusk.process-output")) == 1)

    running.cancel()
    with pytest.raises(asyncio.CancelledError):
        await running

    assert producer.results()[-1]["status"] == "ended"
    assert node.commands_in(DEFAULT_SH_PID)[-1] == f"kill {PID:#x}; ps"
    assert node.processes[PID] == ["sh[server]", "Z"]


async def test_work_dawn_stops_after_its_result_keeps_its_result(producer, fleet, node):
    node.commands["logs"] = [Wait(30)]
    running = asyncio.create_task(
        runner(producer, fleet).run(
            node_ref(),
            work_spec(stream_logs={"level": "info", "duration_seconds": 600}),
        )
    )
    await until(
        lambda: (
            "logs stream otlp://otel-collector:4317 -l info" in node.commands_in(PID)
        )
    )

    running.cancel()
    with pytest.raises(asyncio.CancelledError):
        await running

    finished = producer.results()[-1]
    assert finished["status"] == "succeeded"
    assert finished["error"] is None


async def test_a_shell_at_the_pid_that_cannot_be_stopped_is_logged(
    producer, fleet, node, caplog: pytest.LogCaptureFixture
):
    node.scripts[f"kill {PID:#x}; ps"] = [RuntimeError("Disconnected: gone")]

    assert await run(producer, fleet) == "succeeded"

    assert node.processes[PID] == ["sh[server]", "RR"]
    assert (
        "could not stop the work's shell server; it runs until it is reaped"
        in caplog.messages
    )


async def test_work_that_cannot_be_run_reports_unreachable(producer, fleet):
    await runner(producer, fleet).unreachable(
        node_ref(), work_spec(), Unreachable("nobody home")
    )

    (finished,) = producer.results()
    assert finished["status"] == "unreachable"
    assert finished["delivered"] is False
    assert finished["error"] == "nobody home"
    assert (
        finished["output_digest"]
        == hmac.new(OUTPUT_KEY, digestmod=hashlib.sha256).hexdigest()
    )


@pytest.mark.parametrize(
    ("failure", "delivered", "status"),
    [
        (Unreachable("x"), False, "unreachable"),
        (TimeoutError(), True, "timed_out"),
        (
            RuntimeError(
                "Unimplemented: remote exception: not permitted: Dusk.settime"
            ),
            True,
            "denied",
        ),
        (
            RuntimeError(
                "Unimplemented: remote exception: unknown interface 1 method 0"
            ),
            True,
            "denied",
        ),
        (
            RuntimeError("Unimplemented: remote exception: remote exception: x"),
            True,
            "error",
        ),
        (RuntimeError("Failed: denied: KvsPortal.set"), True, "error"),
        (RuntimeError("Failed: Permission denied (os error 13)"), True, "error"),
        (
            RuntimeError("Failed: Unimplemented: remote exception: not permitted: x"),
            True,
            "error",
        ),
        (RuntimeError("Disconnected: peer hung up"), False, "unreachable"),
        (RuntimeError("disconnected: session revoked"), False, "unreachable"),
        (RuntimeError("Disconnected: peer hung up"), True, "error"),
        (RuntimeError("Connection is closed"), False, "unreachable"),
        (RuntimeError("Connection is closed"), True, "error"),
        (ValueError("anything else"), False, "error"),
    ],
)
def test_failures_map_to_statuses(failure, delivered, status):
    assert classify(failure, delivered) == status


def test_a_batch_runs_quarantine_then_scripts_then_config_then_version():
    work = [
        work_spec(
            pid="70001",
            kind="ensure_version",
            version_key="dusk.version",
            desired_version="1",
        ),
        work_spec(pid="70002", kind="run_script"),
        work_spec(pid="70003", kind="ensure_config", config_hash="x"),
        work_spec(pid="70004", kind="quarantine"),
        work_spec(pid="70005", kind="run_script"),
    ]

    batch = ordered([(node_ref(), each) for each in work])

    assert [each.pid[-1] for _, each in batch] == ["4", "2", "5", "3", "1"]


async def until(condition, seconds: float = 5.0) -> None:
    loop = asyncio.get_running_loop()
    deadline = loop.time() + seconds
    while not condition():
        assert loop.time() < deadline, "the condition never held"
        await asyncio.sleep(0.01)
