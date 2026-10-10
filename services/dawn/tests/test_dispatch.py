from __future__ import annotations

import asyncio

import pytest
from conftest import (
    FakeFleet,
    FakeNode,
    RecordingProducer,
    Wait,
    dawn_settings,
    node_ref,
    work_spec,
)
from pydantic import ValidationError

from dawn.config import KafkaTopics
from dawn.dispatch import Dispatcher, Draining, QueueFull
from dawn.events import Events
from dawn.models import DispatchRequest, ReapRequest
from dawn.nodes import Sessions, SessionsExhausted
from dawn.work import Results, Runner

pytestmark = pytest.mark.anyio


def pid(number: int) -> str:
    return str(0x10000 + number)


def dispatcher(
    fleet: FakeFleet,
    producer: RecordingProducer,
    sessions: Sessions | None = None,
    **limits,
) -> Dispatcher:
    settings = dawn_settings(limits=limits)
    runner = Runner(
        settings, fleet, Results(Events(producer, KafkaTopics(), "dawn-0"), b"k" * 32)
    )
    return Dispatcher(settings, runner, sessions or Sessions(100))


def request(*work, **node_fields) -> DispatchRequest:
    return DispatchRequest(node=node_ref(**node_fields), work=list(work))


def reap(*pids: str, **node_fields) -> ReapRequest:
    return ReapRequest(node=node_ref(**node_fields), pids=list(pids))


def finished(producer: RecordingProducer) -> list[tuple[str, str]]:
    return [
        (message["pid"], message["status"])
        for message in producer.results()
        if message["status"] != "started"
    ]


async def settled(dispatch: Dispatcher) -> None:
    while dispatch.active():
        await asyncio.sleep(0.01)


def scripts(node: FakeNode, *numbers: int) -> list[str]:
    return [
        command for number in numbers for command in node.commands_in(int(pid(number)))
    ]


async def test_a_batch_runs_in_kind_order_each_in_a_shell_server_at_its_pid(
    fleet, producer
):
    node = fleet.add(FakeNode())
    dispatch = dispatcher(fleet, producer)

    accepted = dispatch.submit(
        request(
            work_spec(
                pid=pid(1),
                kind="ensure_version",
                script="echo version",
                version_key="dusk.version",
                desired_version="9",
            ),
            work_spec(pid=pid(2), script="echo script"),
            work_spec(pid=pid(3), kind="quarantine", script="echo quarantine"),
        )
    )
    await settled(dispatch)

    assert accepted == [pid(1), pid(2), pid(3)]
    shells = [
        shell
        for shell in node.shells()
        if shell in {int(pid(1)), int(pid(2)), int(pid(3))}
    ]
    assert shells == [int(pid(3)), int(pid(2)), int(pid(1))]
    assert scripts(node, 3, 2) == ["echo quarantine", "echo script"]
    assert scripts(node, 1)[0] == "echo version"
    for number in (1, 2, 3):
        assert node.processes[int(pid(number))] == ["sh[server]", "Z"]
    assert finished(producer) == [
        (pid(3), "succeeded"),
        (pid(2), "succeeded"),
        (pid(1), "succeeded"),
    ]


async def test_dispatches_that_arrive_while_one_runs_are_coalesced_into_the_next_batch(
    fleet, producer
):
    node = fleet.add(FakeNode())
    node.scripts["slow"] = [Wait(0.2)]
    dispatch = dispatcher(fleet, producer)

    dispatch.submit(request(work_spec(pid=pid(1), script="slow")))
    await asyncio.sleep(0.05)
    dispatch.submit(
        request(work_spec(pid=pid(2), kind="ensure_config", config_hash="x"))
    )
    dispatch.submit(request(work_spec(pid=pid(3), kind="quarantine")))
    await settled(dispatch)

    assert [each for each, _ in finished(producer)] == [pid(1), pid(3), pid(2)]


async def test_a_pid_already_held_is_accepted_again_but_runs_once(fleet, producer):
    node = fleet.add(FakeNode())
    node.scripts["slow"] = [Wait(0.1)]
    dispatch = dispatcher(fleet, producer)

    dispatch.submit(request(work_spec(pid=pid(1), script="slow")))
    assert dispatch.submit(request(work_spec(pid=pid(1), script="slow"))) == [pid(1)]
    await settled(dispatch)

    assert scripts(node, 1) == ["slow"]
    assert finished(producer) == [(pid(1), "succeeded")]


async def test_work_resent_after_its_result_is_a_duplicate(fleet, producer):
    node = fleet.add(FakeNode())
    dispatch = dispatcher(fleet, producer)

    dispatch.submit(request(work_spec(pid=pid(1))))
    await settled(dispatch)
    dispatch.submit(request(work_spec(pid=pid(1))))
    await settled(dispatch)

    assert scripts(node, 1) == ["echo hello"]
    assert finished(producer) == [(pid(1), "succeeded"), (pid(1), "duplicate")]


def test_a_dispatch_names_each_pid_once_and_at_least_one():
    with pytest.raises(ValidationError, match="names a pid twice"):
        request(work_spec(pid=pid(1)), work_spec(pid=pid(1)))
    with pytest.raises(ValidationError, match="at least 1 item"):
        request()


async def test_the_queue_of_a_busy_node_is_bounded(fleet, producer):
    node = fleet.add(FakeNode())
    node.scripts["slow"] = [Wait(0.2)]
    dispatch = dispatcher(fleet, producer, max_work_per_node=2)
    dispatch.submit(request(work_spec(pid=pid(1), script="slow")))
    await asyncio.sleep(0.05)
    dispatch.submit(request(work_spec(pid=pid(2)), work_spec(pid=pid(3))))

    with pytest.raises(QueueFull):
        dispatch.submit(request(work_spec(pid=pid(4))))
    await settled(dispatch)


async def test_a_dispatch_larger_than_a_nodes_queue_is_malformed(fleet, producer):
    dispatch = dispatcher(fleet, producer, max_work_per_node=1)

    with pytest.raises(ValueError, match="limits.max_work_per_node"):
        dispatch.submit(request(work_spec(pid=pid(1)), work_spec(pid=pid(2))))


async def test_scripts_waiting_to_run_are_bounded_in_bytes(fleet, producer):
    node = fleet.add(FakeNode())
    big = "echo " + "x" * 600_000
    node.scripts[big] = [Wait(0.2)]
    dispatch = dispatcher(fleet, producer, max_queued_script_bytes=1048576)

    dispatch.submit(request(work_spec(pid=pid(1), script=big)))
    with pytest.raises(QueueFull, match="limits.max_queued_script_bytes"):
        dispatch.submit(request(work_spec(pid=pid(2), script=big), device_id="9" * 32))
    await settled(dispatch)

    dispatch.submit(request(work_spec(pid=pid(3), script=big), device_id="9" * 32))
    await settled(dispatch)


async def test_different_nodes_run_at_the_same_time(fleet, producer):
    first = fleet.add(FakeNode(namespace_id="0000000000000001"))
    second = fleet.add(
        FakeNode(
            device_id="1" * 32,
            installation_id="2" * 32,
            namespace_id="0000000000000002",
        )
    )
    first.scripts["slow"] = [Wait(0.3)]
    second.scripts["slow"] = [Wait(0.3)]
    dispatch = dispatcher(fleet, producer)
    loop = asyncio.get_running_loop()
    began = loop.time()

    dispatch.submit(
        request(work_spec(pid=pid(1), script="slow"), namespace_id="0000000000000001")
    )
    dispatch.submit(
        request(
            work_spec(pid=pid(2), script="slow"),
            device_id="1" * 32,
            installation_id="2" * 32,
            namespace_id="0000000000000002",
        )
    )
    await settled(dispatch)

    assert loop.time() - began < 0.55
    assert {status for _, status in finished(producer)} == {"succeeded"}


async def test_all_work_for_a_node_nightfall_does_not_hold_is_unreachable(
    fleet, producer
):
    dispatch = dispatcher(fleet, producer)

    dispatch.submit(request(work_spec(pid=pid(1)), work_spec(pid=pid(2))))
    await settled(dispatch)

    assert finished(producer) == [(pid(1), "unreachable"), (pid(2), "unreachable")]


async def test_a_node_session_is_held_while_its_dispatch_runs_and_released_after(
    fleet, producer
):
    node = fleet.add(FakeNode())
    node.scripts["slow"] = [Wait(0.2)]
    sessions = Sessions(1)
    dispatch = dispatcher(fleet, producer, sessions=sessions)

    dispatch.submit(request(work_spec(pid=pid(1), script="slow")))

    assert sessions.open == 1
    with pytest.raises(SessionsExhausted):
        dispatch.submit(request(work_spec(pid=pid(2)), device_id="9" * 32))
    await settled(dispatch)
    assert sessions.open == 0


async def test_reaps_run_before_the_work_of_their_batch(fleet, producer):
    node = fleet.add(FakeNode())
    reaped = int(pid(7))
    node.processes[reaped] = ["sh[server]", "RR"]
    node.scripts["slow"] = [Wait(0.1)]
    dispatch = dispatcher(fleet, producer)

    dispatch.submit(request(work_spec(pid=pid(8), script="slow")))
    await asyncio.sleep(0.02)
    dispatch.submit(request(work_spec(pid=pid(1))))
    accepted = dispatch.submit_reap(reap(pid(7), pid(7)))
    await settled(dispatch)

    assert accepted == [pid(7)]
    assert reaped not in node.processes
    assert finished(producer) == [
        (pid(8), "succeeded"),
        (pid(7), "reaped"),
        (pid(1), "succeeded"),
    ]


async def test_reaps_waiting_for_a_node_are_bounded(fleet, producer):
    node = fleet.add(FakeNode())
    node.scripts["slow"] = [Wait(0.2)]
    dispatch = dispatcher(fleet, producer, max_work_per_node=1)
    dispatch.submit(request(work_spec(pid=pid(1), script="slow")))
    dispatch.submit_reap(reap(pid(2)))

    with pytest.raises(QueueFull, match="reaps waiting"):
        dispatch.submit_reap(reap(pid(3)))
    await settled(dispatch)


async def test_draining_refuses_new_dispatches_and_waits_for_running_ones(
    fleet, producer
):
    node = fleet.add(FakeNode())
    node.scripts["slow"] = [Wait(0.2)]
    dispatch = dispatcher(fleet, producer)
    dispatch.submit(request(work_spec(pid=pid(1), script="slow")))

    draining = asyncio.ensure_future(dispatch.drain(5))
    await asyncio.sleep(0)
    with pytest.raises(Draining):
        dispatch.submit(request(work_spec(pid=pid(2))))
    with pytest.raises(Draining):
        dispatch.submit_reap(reap(pid(3)))
    await draining

    assert finished(producer) == [(pid(1), "succeeded")]


async def test_a_drain_that_runs_out_of_time_reports_all_work_it_stopped(
    fleet, producer, caplog: pytest.LogCaptureFixture
):
    node = fleet.add(FakeNode())
    node.scripts["slow"] = [Wait(30)]
    dispatch = dispatcher(fleet, producer)
    dispatch.submit(
        request(
            work_spec(pid=pid(1), script="slow"),
            work_spec(pid=pid(2), kind="ensure_config", config_hash="x"),
        )
    )
    await asyncio.sleep(0.05)
    dispatch.submit(request(work_spec(pid=pid(3))))
    dispatch.submit_reap(reap(pid(7)))

    await dispatch.drain(0.1)

    results = {
        message["pid"]: message
        for message in producer.results()
        if message["status"] != "started"
    }
    assert set(results) == {pid(1), pid(2), pid(3)}
    assert results[pid(1)]["status"] == "running"
    assert results[pid(1)]["delivered"] is True
    assert results[pid(1)]["error"] == "dawn stopped before the script ended"
    assert results[pid(2)]["status"] == results[pid(3)]["status"] == "error"
    assert "dawn stopped before running the process" == results[pid(2)]["error"]
    assert results[pid(3)]["delivered"] is False
    assert "dawn stopped before reaping processes on a node" in caplog.messages
    assert dispatch.active() == 0
