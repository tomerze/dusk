from __future__ import annotations

import pytest
from conftest import PROGRAMS, FakeConnectionFactory
from fastapi import HTTPException, Request
from starlette.testclient import TestClient


def owner_from_header(request: Request) -> object:
    return request.headers["x-owner"]


@pytest.fixture
def rest_client(connection_factory: FakeConnectionFactory):
    from dusk.gw import ConnectionRegistry
    from dusk.gw.rest import build_application

    registry = ConnectionRegistry(connection_factory)
    api = build_application(registry, PROGRAMS, owner=owner_from_header)
    with TestClient(api) as client:
        yield client


def open_as(client: TestClient, owner: str) -> str:
    response = client.post(
        "/connect", json={"host": "10.0.0.1", "port": 9090}, headers={"x-owner": owner}
    )
    assert response.status_code == 200, response.text
    return response.json()["descriptor"]


def test_a_descriptor_is_reachable_only_by_the_owner_that_opened_it(
    rest_client: TestClient, connection_factory: FakeConnectionFactory
):
    descriptor = open_as(rest_client, "first")

    stranger = rest_client.post(
        "/sh",
        json={"descriptor": descriptor, "command": "ps"},
        headers={"x-owner": "second"},
    )
    owner = rest_client.post(
        "/sh",
        json={"descriptor": descriptor, "command": "ps"},
        headers={"x-owner": "first"},
    )

    assert stranger.status_code == 404
    assert owner.status_code == 200, owner.text
    assert connection_factory.connections[0].commands == ["ps"]


def test_a_stranger_cannot_disconnect_another_owners_descriptor(
    rest_client: TestClient, connection_factory: FakeConnectionFactory
):
    descriptor = open_as(rest_client, "first")

    response = rest_client.post(
        "/disconnect", json={"descriptor": descriptor}, headers={"x-owner": "second"}
    )

    assert response.status_code == 404
    assert connection_factory.connections[0].disconnected is False


def test_a_stranger_cannot_stream_from_another_owners_descriptor(
    rest_client: TestClient,
):
    descriptor = open_as(rest_client, "first")

    response = rest_client.post(
        "/sh/stream",
        json={"descriptor": descriptor, "command": "ps"},
        headers={"x-owner": "second"},
    )

    assert response.status_code == 404


@pytest.mark.parametrize(
    ("path", "body"),
    [
        ("/connect", {"host": "10.0.0.1", "port": 9090}),
        ("/disconnect", {"descriptor": "0000abcd"}),
        ("/sh", {"descriptor": "0000abcd", "command": "ps"}),
        ("/sh/stream", {"descriptor": "0000abcd", "command": "ps"}),
    ],
)
def test_an_owner_that_refuses_the_request_answers_for_itself(
    connection_factory: FakeConnectionFactory, path: str, body: dict
):
    from dusk.gw import ConnectionRegistry
    from dusk.gw.rest import build_application

    def refuse(request: Request) -> object:
        raise HTTPException(401, "not signed in")

    api = build_application(
        ConnectionRegistry(connection_factory), PROGRAMS, owner=refuse
    )
    with TestClient(api) as client:
        response = client.post(path, json=body)

    assert response.status_code == 401
    assert response.json() == {"error": "not signed in"}
    assert connection_factory.connections == []
