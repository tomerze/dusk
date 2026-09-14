"""`POST /v1/sh/stream`: a program's output as it is produced.

The claim worth testing is not that the events are well-formed - it is that a
reader gets each value while the program is still running. `SlowConnection`
below releases a value only when the test says so, so a test that reads one
event before releasing the next has proved the gateway is not waiting for the
command to finish.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import socket
import threading

import httpx
import pytest
import uvicorn
from conftest import PROGRAMS, FakeConnectionFactory
from starlette.testclient import TestClient


def events(body: str) -> list[tuple[str, object]]:
    """Parse a whole SSE body into (event, data) pairs."""
    parsed = []
    for block in body.strip().split("\n\n"):
        if not block.strip():
            continue
        fields = dict(
            line.split(": ", 1) for line in block.splitlines() if ": " in line
        )
        parsed.append((fields["event"], json.loads(fields["data"])))
    return parsed


def read_event(lines) -> tuple[str, object]:
    """Read one complete event off an iterator of lines, as it arrives."""
    event = None
    for line in lines:
        if line.startswith("event: "):
            event = line.removeprefix("event: ")
        elif line.startswith("data: "):
            assert event is not None
            return event, json.loads(line.removeprefix("data: "))
    raise AssertionError("the stream ended before a complete event")


class SlowOutput:
    """Awaits a value that only appears when the test releases it.

    The gate is a ``threading.Event`` because the test sets it from its own
    thread while the gateway runs in another, and it is polled rather than
    waited on because blocking here would block the event loop - which is the
    property this file exists to check.
    """

    def __init__(self, connection: "SlowConnection") -> None:
        self._connection = connection
        self._index = 0

    async def next_value(self):
        if self._index >= len(self._connection.values):
            raise StopAsyncIteration
        gate = self._connection.gates[self._index]
        waited = 0.0
        while not gate.is_set():
            await asyncio.sleep(0.005)
            waited += 0.005
            assert waited < 10, "value was never released"
        value = self._connection.values[self._index]
        self._index += 1
        return value


class SlowConnection:
    """A connection whose values are released one at a time, by the test.

    ``sh`` returns an iterator that blocks per value, the way the real
    ``ShellOutput`` blocks on the channel the node fills.
    """

    def __init__(self, values: list[object]) -> None:
        self.values = values
        self.gates = [threading.Event() for _ in values]
        self.command: str | None = None
        self.disconnected = False

    def sh(self, command: str) -> "SlowOutput":
        self.command = command
        return SlowOutput(self)

    def release(self, index: int) -> None:
        self.gates[index].set()

    def disconnect(self) -> None:
        self.disconnected = True


@pytest.fixture
def slow_connection() -> SlowConnection:
    return SlowConnection(["first", "second"])


@pytest.fixture
def slow_gateway(slow_connection: SlowConnection):
    """A real server on a real socket, whose connections the test controls.

    Not TestClient: it collects a whole response before handing it back, so a
    stream read through it cannot show that anything arrived early - which is
    the one thing worth proving here. Every other test in this file uses
    TestClient, because buffering does not change what the events say.
    """
    import dusk.gw

    application = dusk.gw.app(
        "127.0.0.1",
        connection_factory=lambda host, port: slow_connection,
        programs=PROGRAMS,
    )
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]

    server = uvicorn.Server(
        uvicorn.Config(application, host="127.0.0.1", port=port, log_level="warning")
    )
    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()
    try:
        for _ in range(200):
            if server.started:
                break
            threading.Event().wait(0.05)
        else:
            raise AssertionError("the gateway never started")
        yield f"http://127.0.0.1:{port}"
    finally:
        server.should_exit = True
        thread.join(timeout=10)


def open_connection(client: TestClient) -> str:
    response = client.post("/v1/connect", json={"host": "10.0.0.1", "port": 9090})
    assert response.status_code == 200, response.text
    return response.json()["descriptor"]


def test_a_value_arrives_before_the_command_has_finished(
    slow_gateway: str, slow_connection: SlowConnection
):
    with httpx.Client(base_url=slow_gateway, timeout=30) as client:
        descriptor = client.post(
            "/v1/connect", json={"host": "10.0.0.1", "port": 9090}
        ).json()["descriptor"]

        with client.stream(
            "POST", "/v1/sh/stream", json={"descriptor": descriptor, "command": "logs"}
        ) as response:
            assert response.status_code == 200
            lines = response.iter_lines()
            # Arrives before the program has produced anything, which is what
            # lets this test release values one at a time from here.
            assert read_event(lines) == ("start", {})

            slow_connection.release(0)
            assert read_event(lines) == ("output", "first")

            # The first value has been read and the second does not exist yet.
            # Nothing about this command has finished.
            slow_connection.release(1)
            assert read_event(lines) == ("output", "second")
            assert read_event(lines) == ("end", {})

    assert slow_connection.command == "logs"


def test_a_reader_who_leaves_does_not_hold_the_gateway(
    slow_gateway: str, slow_connection: SlowConnection
):
    """Disconnecting mid-stream must not wedge the server for the next caller."""
    with httpx.Client(base_url=slow_gateway, timeout=30) as client:
        descriptor = client.post(
            "/v1/connect", json={"host": "10.0.0.1", "port": 9090}
        ).json()["descriptor"]

        with contextlib.suppress(httpx.ReadError, httpx.RemoteProtocolError):
            with client.stream(
                "POST",
                "/v1/sh/stream",
                json={"descriptor": descriptor, "command": "logs"},
            ) as response:
                assert read_event(response.iter_lines()) == ("start", {})
                # Leave without ever releasing a value: the command is still
                # parked, exactly like a `logs` follow nobody killed.

        assert client.get("/v1/help").status_code == 200


def test_each_value_is_sent_as_an_output_event(
    client: TestClient, connection_factory: FakeConnectionFactory
):
    descriptor = open_connection(client)

    response = client.post(
        "/v1/sh/stream", json={"descriptor": descriptor, "command": "ps"}
    )

    assert response.status_code == 200
    assert response.headers["content-type"].startswith("text/event-stream")
    assert events(response.text) == [
        ("start", {}),
        ("output", {"ran": "ps", "on": "10.0.0.1:9090"}),
        ("end", {}),
    ]
    assert connection_factory.connections[0].commands == ["ps"]


def test_a_failure_partway_through_is_reported_in_the_stream(
    client: TestClient, connection_factory: FakeConnectionFactory
):
    descriptor = open_connection(client)

    def half_then_fail():
        yield "before"
        raise RuntimeError("the node went away")

    connection_factory.connections[0].produces = half_then_fail()

    response = client.post(
        "/v1/sh/stream", json={"descriptor": descriptor, "command": "ps"}
    )

    # The response had already begun, so this cannot be a status code.
    assert response.status_code == 200
    assert events(response.text) == [
        ("start", {}),
        ("output", "before"),
        ("error", {"error": "the node went away"}),
    ]


def test_output_json_cannot_carry_falls_back_to_repr(
    client: TestClient, connection_factory: FakeConnectionFactory
):
    descriptor = open_connection(client)
    circular: dict = {}
    circular["self"] = circular
    connection_factory.connections[0].produces = [circular]

    response = client.post(
        "/v1/sh/stream", json={"descriptor": descriptor, "command": "ps"}
    )

    assert events(response.text) == [
        ("start", {}),
        ("output", repr(circular)),
        ("end", {}),
    ]


def test_an_unknown_descriptor_is_still_a_404(client: TestClient):
    response = client.post(
        "/v1/sh/stream", json={"descriptor": "deadbeef", "command": "ps"}
    )

    assert response.status_code == 404
    assert response.json()["error"]


@pytest.mark.parametrize(
    "body",
    [
        {"descriptor": "a3f91c07"},
        {"command": "ps"},
        {"descriptor": "a3f91c07", "command": "ps", "follow": True},
    ],
)
def test_a_malformed_body_is_a_400(client: TestClient, body: dict):
    response = client.post("/v1/sh/stream", json=body)

    assert response.status_code == 400
    assert "error" in response.json()


def test_the_streaming_endpoint_is_in_the_document(client: TestClient):
    operation = client.get("/v1/openapi.json").json()["paths"]["/sh/stream"]["post"]

    assert set(operation["responses"]) == {"200", "400", "404"}
    assert "text/event-stream" in operation["responses"]["200"]["content"]
