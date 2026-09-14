"""MCP is mounted at ``/mcp`` alongside the REST API, always.

These tests speak streamable HTTP to the endpoint directly rather than through an
MCP client library, so what they assert is what an MCP client would see on the
wire: that the endpoint answers at all, and that it advertises one tool per dusk
program plus the gateway's own connect / disconnect.
"""

from __future__ import annotations

import json
import string

import pytest
from conftest import PROGRAMS, FakeConnectionFactory
from starlette.testclient import TestClient

PROTOCOL_VERSION = "2025-06-18"
ACCEPT_BOTH = "application/json, text/event-stream"


def sole_message(response) -> dict:
    """Read the one JSON-RPC message out of a streamable-HTTP reply.

    The endpoint answers either as a single JSON body or as a one-message SSE
    stream, depending on what it has to say; both carry the same message.
    """
    if response.headers["content-type"].startswith("application/json"):
        return response.json()
    for line in response.text.splitlines():
        if line.startswith("data:"):
            return json.loads(line.removeprefix("data:").strip())
    raise AssertionError(f"no message in response: {response.text!r}")


def call(client: TestClient, method: str, session_id: str | None = None, **params):
    """Send one JSON-RPC request to the MCP endpoint."""
    headers = {"accept": ACCEPT_BOTH, "content-type": "application/json"}
    if session_id is not None:
        headers["mcp-session-id"] = session_id
    return client.post(
        "/mcp",
        headers=headers,
        json={"jsonrpc": "2.0", "id": 1, "method": method, "params": params},
    )


@pytest.fixture
def mcp_session(client: TestClient) -> str:
    """Complete the MCP handshake and return the session id."""
    response = call(
        client,
        "initialize",
        protocolVersion=PROTOCOL_VERSION,
        capabilities={},
        clientInfo={"name": "dusk-gateway-tests", "version": "0"},
    )
    assert response.status_code == 200, response.text
    session_id = response.headers["mcp-session-id"]

    client.post(
        "/mcp",
        headers={
            "accept": ACCEPT_BOTH,
            "content-type": "application/json",
            "mcp-session-id": session_id,
        },
        json={"jsonrpc": "2.0", "method": "notifications/initialized"},
    )
    return session_id


def test_the_mcp_endpoint_is_served_without_being_asked_for(client: TestClient):
    response = call(
        client,
        "initialize",
        protocolVersion=PROTOCOL_VERSION,
        capabilities={},
        clientInfo={"name": "dusk-gateway-tests", "version": "0"},
    )

    assert response.status_code == 200, response.text
    assert sole_message(response)["result"]["serverInfo"]["name"] == "Dusk"


@pytest.mark.parametrize(
    ("served_at", "reached_at"),
    [
        ("0.0.0.0", "http://10.0.0.5:9100"),
        ("0.0.0.0", "http://127.0.0.1:9100"),
        ("127.0.0.1", "http://localhost:9100"),
    ],
)
def test_mcp_answers_clients_that_reach_the_address_it_is_served_on(
    connection_factory: FakeConnectionFactory, served_at: str, reached_at: str
):
    # FastMCP arms DNS-rebinding protection from the address it is given and
    # rejects any mismatched Host with 421. Left to its own default that address
    # is loopback whatever the gateway binds, which turns away every client of a
    # gateway served on 0.0.0.0 - so the address must be threaded through.
    import dusk.gw

    application = dusk.gw.app(
        served_at, connection_factory=connection_factory, programs=PROGRAMS
    )
    with TestClient(application, base_url=reached_at) as client:
        response = call(
            client,
            "initialize",
            protocolVersion=PROTOCOL_VERSION,
            capabilities={},
            clientInfo={"name": "dusk-gateway-tests", "version": "0"},
        )

    assert response.status_code == 200, response.text


def test_every_dusk_program_is_advertised_as_a_tool(
    client: TestClient, mcp_session: str
):
    response = call(client, "tools/list", mcp_session)

    assert response.status_code == 200, response.text
    tools = {tool["name"]: tool for tool in sole_message(response)["result"]["tools"]}

    assert {"connect", "disconnect"} <= tools.keys()
    for program in PROGRAMS:
        assert program["name"] in tools
        assert program["short_description"] in tools[program["name"]]["description"]


def test_program_tools_offer_tasks_and_the_gateway_tools_do_not(
    client: TestClient, mcp_session: str
):
    response = call(client, "tools/list", mcp_session)
    tools = {tool["name"]: tool for tool in sole_message(response)["result"]["tools"]}

    for program in PROGRAMS:
        assert tools[program["name"]]["execution"]["taskSupport"] == "optional"
    assert tools["connect"].get("execution") is None


def test_a_tool_call_runs_the_command_on_the_node(client: TestClient, mcp_session: str):
    opened = call(
        client,
        "tools/call",
        mcp_session,
        name="connect",
        arguments={"host": "10.0.0.1", "port": 9090},
    )
    descriptor = sole_message(opened)["result"]["content"][0]["text"]

    assert len(descriptor) == 8
    assert set(descriptor) <= set(string.hexdigits)

    ran = call(
        client,
        "tools/call",
        mcp_session,
        name="ps",
        arguments={"descriptor": descriptor},
    )

    assert json.loads(sole_message(ran)["result"]["content"][0]["text"]) == {
        "ran": "ps",
        "on": "10.0.0.1:9090",
    }


def test_a_rest_descriptor_is_not_usable_from_an_mcp_session(
    client: TestClient, mcp_session: str
):
    # The registry keys connections by the owner that opened them, so the two
    # surfaces cannot reach into each other's connections even though they share
    # one registry and one descriptor format.
    minted_over_rest = client.post(
        "/v1/connect", json={"host": "10.0.0.1", "port": 9090}
    ).json()["descriptor"]

    response = call(
        client,
        "tools/call",
        mcp_session,
        name="ps",
        arguments={"descriptor": minted_over_rest},
    )

    assert sole_message(response)["result"]["isError"] is True


def test_the_registry_scopes_descriptors_to_their_owner(
    connection_factory: FakeConnectionFactory,
):
    import dusk.gw

    registry = dusk.gw.ConnectionRegistry(connection_factory)
    first_owner = object()
    second_owner = object()

    descriptor, is_first_connection = registry.connect(first_owner, "10.0.0.1", 9090)
    assert is_first_connection is True

    assert registry.get(first_owner, descriptor) is not None
    with pytest.raises(KeyError):
        registry.get(second_owner, descriptor)
    with pytest.raises(KeyError):
        registry.get(dusk.gw.REST_OWNER, descriptor)

    _, is_first_connection = registry.connect(first_owner, "10.0.0.1", 9090)
    assert is_first_connection is False


def test_descriptors_are_unique_across_owners(
    connection_factory: FakeConnectionFactory,
):
    # Uniqueness has to hold registry-wide, not per owner: minting one that some
    # other owner already holds would be invisible here - lookups are scoped -
    # and would leave two connections sharing a name for whoever untangles it.
    import dusk.gw

    registry = dusk.gw.ConnectionRegistry(connection_factory)
    owners = [object(), object(), dusk.gw.REST_OWNER]

    descriptors = [
        registry.connect(owners[index % len(owners)], "10.0.0.1", 9090)[0]
        for index in range(200)
    ]

    assert len(set(descriptors)) == len(descriptors)


def test_disconnecting_an_owner_closes_only_its_own_connections(
    connection_factory: FakeConnectionFactory,
):
    import dusk.gw

    registry = dusk.gw.ConnectionRegistry(connection_factory)
    first_owner = object()
    second_owner = object()

    kept_descriptor, _ = registry.connect(second_owner, "10.0.0.2", 9090)
    registry.connect(first_owner, "10.0.0.1", 9090)
    kept, dropped = connection_factory.connections

    registry.disconnect_owner(first_owner)

    assert dropped.disconnected is True
    assert kept.disconnected is False

    registry.disconnect_all()

    assert kept.disconnected is True
    with pytest.raises(KeyError):
        registry.get(second_owner, kept_descriptor)
