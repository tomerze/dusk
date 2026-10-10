from __future__ import annotations

import asyncio
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import pytest
from conftest import DEFAULT_SH_PID, PID, FakeFleet, FakeNode, dawn_settings, node_ref

from dawn import nodes
from dawn.nodes import Sessions, SessionsExhausted, Unreachable, connect, target_for

pytestmark = pytest.mark.anyio


def test_the_target_names_the_namespace_by_sni_under_the_suffix():
    settings = dawn_settings(nightfall={"server_name_suffix": "fleet.example.org"})

    target = target_for(settings, node_ref(namespace_id="0123456789abcdef"))

    assert target.server_name == "0123456789abcdef.fleet.example.org"
    assert (target.host, target.port) == ("nightfall-inner", 8444)
    assert target.ca == Path("/etc/dawn/pki/internal-ca.crt")
    assert target.certificate == Path("/etc/dawn/tls/client.crt")
    assert target.key == Path("/etc/dawn/tls/client.key")


def test_a_node_reference_may_name_its_nightfall():
    target = target_for(
        dawn_settings(), node_ref(nightfall="nightfall-2.nightfall-inner.dusk.svc:8444")
    )

    assert (target.host, target.port) == ("nightfall-2.nightfall-inner.dusk.svc", 8444)


async def test_a_connection_without_a_pid_is_on_the_default_shell_server(
    fleet: FakeFleet,
):
    node = fleet.add(FakeNode())

    connection = await connect(fleet, dawn_settings(), node_ref(), None)

    assert node.calls == [("connect", DEFAULT_SH_PID)]
    assert [value async for value in values(connection.sh("echo hello"))] == ["hello"]
    assert node.calls[-1] == ("sh", DEFAULT_SH_PID, "echo hello")


async def test_a_connection_at_a_pid_starts_a_shell_server_there(fleet: FakeFleet):
    node = fleet.add(FakeNode())

    await connect(fleet, dawn_settings(), node_ref(), PID)

    assert node.processes[PID] == ["sh[server]", "RR"]
    assert node.calls == [("connect", PID)]


async def test_a_node_nightfall_does_not_hold_is_unreachable(fleet: FakeFleet):
    with pytest.raises(Unreachable, match="^Disconnected: node .* is not connected"):
        await connect(fleet, dawn_settings(), node_ref(), PID)


async def test_a_connection_nightfall_refuses_is_a_denial(fleet: FakeFleet):
    fleet.add(FakeNode()).refusal = RuntimeError(
        "Unimplemented: remote exception: not permitted: Dusk.process"
    )

    with pytest.raises(RuntimeError, match="^Unimplemented: remote exception: not "):
        await connect(fleet, dawn_settings(), node_ref(), PID)


async def test_a_process_nightfall_did_not_admit_is_a_denial(fleet: FakeFleet):
    fleet.add(FakeNode()).refusal = RuntimeError(
        "Failed: remote exception: denied: not intended"
    )

    with pytest.raises(RuntimeError, match="denied: not intended$"):
        await connect(fleet, dawn_settings(), node_ref(), PID)


async def test_a_connection_that_does_not_come_up_in_time_is_closed_once_it_does(
    fleet: FakeFleet,
):
    node = fleet.add(FakeNode())
    fleet.delay = 0.3

    with pytest.raises(Unreachable, match="within 0.05 s"):
        await connect(
            fleet,
            dawn_settings(nightfall={"connect_timeout_seconds": 0.05}),
            node_ref(),
            PID,
        )
    await until(lambda: node.connections and node.connections[0].disconnected)


async def test_a_connection_waiting_for_a_thread_gets_its_whole_time_once_it_has_one(
    fleet: FakeFleet, monkeypatch: pytest.MonkeyPatch
):
    monkeypatch.setattr(nodes, "CONNECTS", ThreadPoolExecutor(max_workers=1))
    node = fleet.add(FakeNode())
    fleet.delay = 0.15
    settings = dawn_settings(nightfall={"connect_timeout_seconds": 0.25})

    await asyncio.gather(
        connect(fleet, settings, node_ref(), PID),
        connect(fleet, settings, node_ref(), None),
    )

    assert len(node.connections) == 2


async def test_no_free_thread_in_time_is_dawn_at_capacity_not_an_unreachable_node(
    fleet: FakeFleet, monkeypatch: pytest.MonkeyPatch
):
    monkeypatch.setattr(nodes, "CONNECTS", ThreadPoolExecutor(max_workers=1))
    node = fleet.add(FakeNode())
    fleet.delay = 0.4
    settings = dawn_settings(nightfall={"connect_timeout_seconds": 0.1})

    with pytest.raises(Unreachable) as abandoned:
        await connect(fleet, settings, node_ref(), PID)
    with pytest.raises(SessionsExhausted, match="none of them finished within 0.1 s"):
        await connect(fleet, settings, node_ref(), None)

    assert abandoned.value.late is not None
    await until(lambda: node.connections and node.connections[0].disconnected)
    assert node.shells() == [PID]


async def test_a_connection_cancelled_while_it_comes_up_is_closed_once_it_does(
    fleet: FakeFleet,
):
    node = fleet.add(FakeNode())
    fleet.delay = 0.2
    connecting = asyncio.ensure_future(connect(fleet, dawn_settings(), node_ref(), PID))
    await asyncio.sleep(0.05)

    connecting.cancel()
    with pytest.raises(asyncio.CancelledError):
        await connecting
    await until(lambda: node.connections and node.connections[0].disconnected)


async def test_closing_a_connection_disconnects_it_once(fleet: FakeFleet):
    node = fleet.add(FakeNode())
    connection = await connect(fleet, dawn_settings(), node_ref(), PID)

    await connection.close()
    await connection.close()

    assert node.calls.count(("disconnect", PID)) == 1


async def test_a_disconnect_that_fails_is_logged_not_raised(
    fleet: FakeFleet, caplog: pytest.LogCaptureFixture
):
    node = fleet.add(FakeNode())
    connection = await connect(fleet, dawn_settings(), node_ref(), PID)
    node.connections[0].disconnected = True

    await connection.close()

    assert "a node connection did not close cleanly" in caplog.messages


class Hanging:
    def sh(self, command: str):
        raise AssertionError(command)

    def disconnect(self) -> None:
        time.sleep(0.5)


async def test_a_disconnect_that_hangs_is_left_to_finish_on_its_own(
    monkeypatch: pytest.MonkeyPatch, caplog: pytest.LogCaptureFixture
):
    monkeypatch.setattr(nodes, "DISCONNECT_SECONDS", 0.05)
    connection = nodes.Connection(Hanging(), {"sh_server_pid": f"{PID:x}"})
    began = time.monotonic()

    await connection.close()

    assert time.monotonic() - began < 0.4
    assert "a node connection did not close in time; it goes on closing" in (
        caplog.messages
    )


def test_sessions_are_bounded_and_counted():
    counts: list[int] = []
    sessions = Sessions(2, counts.append)

    sessions.reserve()
    sessions.reserve()
    with pytest.raises(SessionsExhausted):
        sessions.reserve()
    sessions.release()
    sessions.reserve()

    assert counts == [1, 2, 1, 2]
    assert sessions.open == 2


async def values(output):
    while True:
        try:
            yield await output.next_value()
        except StopAsyncIteration:
            return


async def until(condition, seconds: float = 2.0) -> None:
    deadline = time.monotonic() + seconds
    while not condition():
        assert time.monotonic() < deadline, "the condition never held"
        await asyncio.sleep(0.01)
