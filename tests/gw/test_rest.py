"""The ``/v1`` REST API: one endpoint per method of the ``Dusk`` Python class."""

from __future__ import annotations

import string

import pytest
from conftest import BASE_URL, PROGRAMS, FakeConnectionFactory
from starlette.testclient import TestClient


def open_connection(
    client: TestClient, host: str = "10.0.0.1", port: int = 9090
) -> str:
    """Run the connect endpoint and return the descriptor it minted."""
    response = client.post("/v1/connect", json={"host": host, "port": port})
    assert response.status_code == 200, response.text
    return response.json()["descriptor"]


def test_connect_opens_the_node_and_returns_a_descriptor(
    client: TestClient, connection_factory: FakeConnectionFactory
):
    descriptor = open_connection(client, "10.0.0.1", 9090)

    assert len(descriptor) == 8
    assert set(descriptor) <= set(string.hexdigits)
    assert len(connection_factory.connections) == 1
    assert connection_factory.connections[0].host == "10.0.0.1"
    assert connection_factory.connections[0].port == 9090


def test_a_descriptor_says_nothing_about_the_node_it_reaches(client: TestClient):
    # It is a handle, not an address. Nothing may read a node out of it, and
    # nothing downstream may come to depend on being able to.
    descriptor = open_connection(client, "10.0.0.1", 9090)

    assert "10.0.0.1" not in descriptor
    assert "9090" not in descriptor


def test_repeat_connections_to_one_node_get_distinct_descriptors(client: TestClient):
    first = open_connection(client)
    second = open_connection(client)

    assert first != second


def test_sh_runs_the_command_on_the_descriptor_it_was_given(
    client: TestClient, connection_factory: FakeConnectionFactory
):
    first = open_connection(client, "10.0.0.1", 9090)
    second = open_connection(client, "10.0.0.2", 9090)

    response = client.post("/v1/sh", json={"descriptor": second, "command": "ps"})

    assert response.status_code == 200, response.text
    assert response.json() == {"output": [{"ran": "ps", "on": "10.0.0.2:9090"}]}
    assert connection_factory.connections[0].commands == []
    assert connection_factory.connections[1].commands == ["ps"]
    assert first != second


def test_disconnect_closes_the_connection_and_invalidates_the_descriptor(
    client: TestClient, connection_factory: FakeConnectionFactory
):
    descriptor = open_connection(client)

    response = client.post("/v1/disconnect", json={"descriptor": descriptor})
    assert response.status_code == 200, response.text
    assert response.json() == {"descriptor": descriptor}
    assert connection_factory.connections[0].disconnected is True

    reused = client.post("/v1/sh", json={"descriptor": descriptor, "command": "ps"})
    assert reused.status_code == 404
    assert descriptor in reused.json()["error"]


def test_help_reports_the_nodes_program_set(client: TestClient):
    response = client.get("/v1/help")

    assert response.status_code == 200
    assert response.json() == PROGRAMS


def test_help_for_one_program_reports_just_that_program(client: TestClient):
    response = client.get("/v1/help/sleep")

    assert response.status_code == 200
    assert response.json()["name"] == "sleep"
    assert response.json()["long_description"] == "Usage: sleep <seconds>"


def test_help_for_an_unknown_program_is_a_404(client: TestClient):
    response = client.get("/v1/help/rm")

    assert response.status_code == 404
    assert "rm" in response.json()["error"]


def test_a_node_that_refuses_the_connection_is_a_502(
    client: TestClient, connection_factory: FakeConnectionFactory
):
    connection_factory.failure = ConnectionRefusedError("connection refused")

    response = client.post("/v1/connect", json={"host": "10.0.0.1", "port": 9090})

    assert response.status_code == 502
    assert "10.0.0.1:9090" in response.json()["error"]


@pytest.mark.parametrize(
    ("path", "body"),
    [
        ("/v1/connect", {"port": 9090}),
        ("/v1/connect", {"host": "10.0.0.1"}),
        ("/v1/connect", {"host": "10.0.0.1", "port": "9090"}),
        ("/v1/connect", {"host": 10, "port": 9090}),
        ("/v1/disconnect", {}),
        ("/v1/sh", {"descriptor": "a3f91c07"}),
        ("/v1/sh", {"descriptor": "a3f91c07", "command": 7}),
    ],
)
def test_a_malformed_body_is_a_400(client: TestClient, path: str, body: dict):
    response = client.post(path, json=body)

    assert response.status_code == 400, response.text
    assert "error" in response.json()


def test_a_boolean_is_not_accepted_as_a_port(client: TestClient):
    # bool subclasses int in Python, so an unguarded isinstance check would read
    # this as port 1 and open a connection to it.
    response = client.post("/v1/connect", json={"host": "10.0.0.1", "port": True})

    assert response.status_code == 400
    assert "port" in response.json()["error"]


def test_a_body_that_is_not_an_object_is_a_400(client: TestClient):
    response = client.post(
        "/v1/connect", content=b"[1, 2]", headers={"content-type": "application/json"}
    )

    assert response.status_code == 400

    not_json = client.post(
        "/v1/connect", content=b"{", headers={"content-type": "application/json"}
    )

    assert not_json.status_code == 400


def test_an_unknown_descriptor_is_a_404(client: TestClient):
    response = client.post("/v1/sh", json={"descriptor": "deadbeef", "command": "ps"})

    assert response.status_code == 404

    disconnect = client.post("/v1/disconnect", json={"descriptor": "deadbeef"})

    assert disconnect.status_code == 404


def test_output_json_cannot_carry_falls_back_to_repr_rather_than_failing(
    client: TestClient, connection_factory: FakeConnectionFactory
):
    descriptor = open_connection(client)
    circular: dict = {}
    circular["self"] = circular
    connection_factory.connections[0].produces = [circular]

    response = client.post("/v1/sh", json={"descriptor": descriptor, "command": "ps"})

    assert response.status_code == 200
    assert response.json()["output"] == [repr(circular)]


def test_a_command_the_node_cannot_run_is_a_502(
    client: TestClient, connection_factory: FakeConnectionFactory
):
    # A node runs each command in a shell task from a bounded pool and answers
    # Busy once they are taken. Letting that escape the handler would answer a
    # plain-text 500 — the one response this API would not render as JSON.
    def busy():
        raise RuntimeError("Busy - Too many instances of this task are already running")
        yield

    descriptor = open_connection(client)
    connection_factory.connections[0].produces = busy()

    response = client.post("/v1/sh", json={"descriptor": descriptor, "command": "ps"})

    assert response.status_code == 502
    assert response.headers["content-type"].startswith("application/json")
    assert "Busy" in response.json()["error"]


def test_the_bare_root_is_not_served(client: TestClient):
    assert client.get("/").status_code == 404


def test_shutting_the_gateway_down_closes_connections_left_open(
    gateway_app, connection_factory: FakeConnectionFactory
):
    with TestClient(gateway_app, base_url=BASE_URL) as client:
        open_connection(client)
        open_connection(client)
        assert [opened.disconnected for opened in connection_factory.connections] == [
            False,
            False,
        ]

    assert [opened.disconnected for opened in connection_factory.connections] == [
        True,
        True,
    ]
