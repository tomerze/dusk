from __future__ import annotations

import json

from conftest import (
    DEVICE_ID,
    INSTALLATION_ID,
    NAMESPACE_ID,
    OPERATOR_TOKEN,
    PID,
    Dawn,
    FakeNode,
    bearer,
)
from starlette.testclient import TestClient

ACCEPT_BOTH = "application/json, text/event-stream"


def sole_message(response) -> dict:
    if response.headers["content-type"].startswith("application/json"):
        return response.json()
    for line in response.text.splitlines():
        if line.startswith("data:"):
            return json.loads(line.removeprefix("data:").strip())
    raise AssertionError(f"no message in response: {response.text!r}")


def call(client: TestClient, method: str, session_id: str | None = None, **params):
    headers = {
        "accept": ACCEPT_BOTH,
        "content-type": "application/json",
        **bearer(OPERATOR_TOKEN),
    }
    if session_id is not None:
        headers["mcp-session-id"] = session_id
    return client.post(
        "/mcp",
        headers=headers,
        json={"jsonrpc": "2.0", "id": 1, "method": method, "params": params},
    )


def session(client: TestClient) -> tuple[str, dict]:
    response = call(
        client,
        "initialize",
        protocolVersion="2025-06-18",
        capabilities={},
        clientInfo={"name": "dawn-tests", "version": "0"},
    )
    session_id = response.headers["mcp-session-id"]
    client.post(
        "/mcp",
        headers={
            "accept": ACCEPT_BOTH,
            "content-type": "application/json",
            "mcp-session-id": session_id,
            **bearer(OPERATOR_TOKEN),
        },
        json={"jsonrpc": "2.0", "method": "notifications/initialized"},
    )
    return session_id, sole_message(response)["result"]


def test_the_mcp_instructions_say_how_to_connect_through_dawn(client: TestClient):
    _, initialized = session(client)

    assert "node reference" in initialized["instructions"]
    assert "host and port" not in initialized["instructions"]


def test_the_connect_tool_takes_a_node_reference_and_a_pid(client: TestClient):
    session_id, _ = session(client)

    tools = sole_message(call(client, "tools/list", session_id))["result"]["tools"]

    connect = next(tool for tool in tools if tool["name"] == "connect")
    assert set(connect["inputSchema"]["properties"]) == {
        "device_id",
        "installation_id",
        "namespace_id",
        "pid",
        "nightfall",
    }
    assert {tool["name"] for tool in tools} == {"connect", "disconnect", "ps"}


def test_an_operator_runs_a_program_through_an_mcp_session(
    dawn: Dawn, client: TestClient
):
    node = dawn.fleet.add(FakeNode())
    session_id, _ = session(client)

    connected = sole_message(
        call(
            client,
            "tools/call",
            session_id,
            name="connect",
            arguments={
                "device_id": DEVICE_ID,
                "installation_id": INSTALLATION_ID,
                "namespace_id": NAMESPACE_ID,
                "pid": str(PID),
            },
        )
    )["result"]
    descriptor = connected["content"][0]["text"]
    ran = sole_message(
        call(
            client,
            "tools/call",
            session_id,
            name="ps",
            arguments={"descriptor": descriptor},
        )
    )["result"]

    assert connected.get("isError") is not True, connected
    assert len(descriptor) == 8
    assert ran.get("isError") is not True, ran
    assert "nightfall" in ran["content"][0]["text"]
    assert node.calls[0] == ("connect", PID)
    assert node.commands_in(PID) == ["ps"]


def test_a_connect_at_a_malformed_pid_is_a_tool_error(dawn: Dawn, client: TestClient):
    dawn.fleet.add(FakeNode())
    session_id, _ = session(client)

    result = sole_message(
        call(
            client,
            "tools/call",
            session_id,
            name="connect",
            arguments={
                "device_id": DEVICE_ID,
                "installation_id": INSTALLATION_ID,
                "namespace_id": NAMESPACE_ID,
                "pid": "0",
            },
        )
    )["result"]

    assert result["isError"] is True
    assert "pid" in result["content"][0]["text"]
    assert dawn.fleet.targets == []
