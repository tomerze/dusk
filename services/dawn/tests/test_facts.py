from __future__ import annotations

import pytest
from conftest import DEFAULT_SH_PID, KVS_TYPE_ID, PS_TYPE_ID, FakeNode

from dawn import facts
from dawn.nodes import Connection

pytestmark = pytest.mark.anyio


def connection(node: FakeNode) -> Connection:
    return Connection(node.connect(None), {})


async def test_every_registered_fact_is_read_and_the_device_id_dropped():
    node = FakeNode()
    node.kvs["dusk.device.memory_bytes"] = 8589934592
    node.registered.add("dusk.device.memory_bytes")
    node.kvs["app.version"] = "3.1.0"

    found, reported = await facts.collect(connection(node), [])

    assert found == {
        "dusk.version": "0.1.0",
        "dusk.os.locale": "en_US.UTF-8",
        "dusk.hostname": "node-1",
        "dusk.device.memory_bytes": 8589934592,
    }
    assert reported == {
        "version_key": "dusk.version",
        "version": "0.1.0",
        "config_hash": None,
        "services": ["nightfall", "sh[server]"],
        "facts": found,
    }
    assert node.commands_in(DEFAULT_SH_PID) == [
        "kvs get dusk.; ps",
        "kvs get dusk.config.hash",
    ]


async def test_version_keys_outside_dusk_are_read_by_their_exact_name():
    node = FakeNode()
    node.kvs["app.version"] = "3.1.0"
    node.kvs["app.version.previous"] = "3.0.0"

    found, reported = await facts.collect(
        connection(node), ["app.version", "app.absent"]
    )

    assert found["app.version"] == "3.1.0"
    assert "app.version.previous" not in found
    assert "app.absent" not in found
    assert reported["version_key"] == "app.version"
    assert reported["version"] == "3.1.0"
    assert node.commands_in(DEFAULT_SH_PID) == [
        "kvs get dusk.; ps",
        "kvs get dusk.config.hash",
        "kvs get app.version",
        "kvs get app.absent",
    ]


async def test_the_config_hash_the_node_never_registered_is_read_by_its_id():
    node = FakeNode()
    node.kvs["dusk.config.hash"] = "ab" * 32

    found, reported = await facts.collect(connection(node), [])

    assert reported["config_hash"] == "ab" * 32
    assert found["dusk.config.hash"] == "ab" * 32


async def test_a_read_answers_rows_by_name_and_processes_by_pid():
    node = FakeNode()

    rows, processes = await facts.read(connection(node), [facts.EVERY_FACT])

    assert facts.value_of(rows, "dusk.version") == "0.1.0"
    assert processes[DEFAULT_SH_PID] == ("sh[server]", "RR")
    assert processes[4] == ("old", "Z")


async def test_a_name_already_read_is_not_read_again():
    node = FakeNode()

    rows, _ = await facts.read(connection(node), [facts.EVERY_FACT], ["dusk.version"])

    assert facts.value_of(rows, "dusk.version") == "0.1.0"
    assert node.commands_in(DEFAULT_SH_PID) == ["kvs get dusk.; ps"]


async def test_a_key_the_node_does_not_have_reads_as_none():
    rows, _ = await facts.read(connection(FakeNode()), [], ["app.version"])

    assert facts.value_of(rows, "app.version") is None


async def test_a_name_whose_read_fails_otherwise_fails_the_read():
    node = FakeNode()
    node.scripts["kvs get app.version"] = RuntimeError("Disconnected: gone")

    with pytest.raises(RuntimeError, match="Disconnected"):
        await facts.read(connection(node), [], ["app.version"])


async def test_a_read_larger_than_its_budget_fails(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setattr(facts, "MAX_READ_BYTES", 64)
    node = FakeNode()
    node.kvs["dusk.large"] = "x" * 100
    node.registered.add("dusk.large")

    with pytest.raises(ValueError, match="more than 64 bytes"):
        await facts.read(connection(node), [facts.EVERY_FACT])


def test_the_key_id_matches_the_kvs_programs_hash():
    assert facts.key_id("dusk.config.hash") == 0xA6174321A8FF3838
    assert facts.key_id("") == 0xCBF29CE484222325 ^ 0x93968E6E30A593D6


def test_rows_that_are_not_lists_are_left_out():
    assert facts.key_rows({"Key": "a", "Value": ["b"]}) == {}
    assert facts.process_rows({"PID": 1, "Name": ["a"], "State": ["R"]}) == {}


def test_only_running_processes_are_services():
    rows = facts.process_rows(
        {
            "PID": [1, 2, 3, 4, 5, 6],
            "Name": ["a", "b", "c", "d", "e", "a"],
            "State": ["R", "RR", "S", "RS", "Z", "RR"],
        }
    )

    assert facts.services(rows) == ["a", "b"]


def test_the_ps_that_listed_the_processes_is_not_a_service():
    rows = facts.process_rows(
        {"PID": [1, 2], "Name": ["nightfall", "ps"], "State": ["RR", "RR"]}
    )

    assert facts.services(rows) == ["nightfall"]


def test_values_that_are_not_records_are_ignored():
    assert facts.record_fields("text") is None
    assert facts.record_fields({"a": 1, "b": 2}) is None
    assert facts.record_fields({"type": {"x": 1}}) is None
    assert facts.record_fields({"0x1": {"x": 1}}) == {"x": 1}


def test_values_are_rendered_as_json_can_hold_them():
    assert facts.renderable(b"\x01") == "b'\\x01'"
    assert facts.renderable({"k": [1, None, True]}) == {"k": [1, None, True]}
    assert facts.renderable({"k": [float("nan"), float("-inf")]}) == {
        "k": ["nan", "-inf"]
    }


async def test_records_of_other_programs_are_not_facts():
    node = FakeNode()
    node.scripts["kvs get dusk.; ps"] = [
        {"0x1234": {"Key": ["dusk.version"], "Other": [1]}},
        {KVS_TYPE_ID: {"Key": ["dusk.version"], "Value": ["2.0.0"]}},
        {PS_TYPE_ID: {"PID": [9], "Name": ["sh"], "State": ["RR"]}},
    ]

    found, reported = await facts.collect(connection(node), [])

    assert found == {"dusk.version": "2.0.0"}
    assert reported["services"] == ["sh"]


async def test_one_key_the_node_does_not_have_does_not_fail_the_read():
    node = FakeNode()

    found, reported = await facts.collect(connection(node), ["app.absent"])

    assert "app.absent" not in found
    assert reported["services"] == ["nightfall", "sh[server]"]


async def test_values_of_keys_nobody_asked_for_are_not_facts():
    node = FakeNode()
    node.kvs["app.secret"] = "s3cr3t"
    node.registered.add("app.secret")
    node.kvs["unregistered"] = "x"

    found, reported = await facts.collect(connection(node), [])

    assert "app.secret" not in found
    assert all(name.startswith("dusk.") for name in found)
    assert "s3cr3t" not in str(reported)
