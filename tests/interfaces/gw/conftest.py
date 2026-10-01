"""Fixtures for the dusk API gateway tests.

These tests drive the gateway's own logic - routing, request validation, the
connection registry, error mapping - and never open a real node connection. The
gateway takes both of its outside dependencies as parameters (``app`` accepts a
``connection_factory`` and a ``programs`` list), so the doubles below are handed
in the same way any other caller would hand in its own.

That keeps the suite runnable from a plain checkout: the ``dusk`` package is
imported from the source tree with its compiled extension replaced by a stub, so
nothing here needs ``maturin develop`` or a built node. The end-to-end coverage
that does need a real node lives in ``tests/interfaces/py``.
"""

from __future__ import annotations

import pathlib
import sys
import types
from typing import Any

import pytest

PYTHON_SOURCE = pathlib.Path(__file__).resolve().parents[3] / "dusk/src/dusk_py/python"

# ``dusk/__init__.py`` is one line: ``from .dusk import *``, importing the
# PyO3 extension. Seeding sys.modules with a stub for it, before anything
# imports dusk, is what lets the gateway be imported without that extension
# being built. The gateway itself only reaches for the real ``Dusk`` class when
# no connection factory was supplied, which these tests always supply.
sys.path.insert(0, str(PYTHON_SOURCE))
sys.modules["dusk.dusk"] = types.ModuleType("dusk.dusk")


PROGRAMS = [
    {
        "name": "ps",
        "version": "0.1.0",
        "short_description": "List the processes on the node.",
        "long_description": "Usage: ps",
        "program_id": 15065078153151533341,
    },
    {
        "name": "sleep",
        "version": "0.1.0",
        "short_description": "Sleep for a duration.",
        "long_description": "Usage: sleep <seconds>",
        "program_id": 10164066927729266340,
    },
]
"""Stand-in for what ``Dusk.help()`` reports, in the shape the real one returns.

The five fields are the ones ``entry_info_to_dict`` sets in ``dusk_py``; the
``Program`` model in :mod:`dusk.gw.rest` names all five, so a double missing one
would fail where the real thing does not."""


class FakeOutput:
    """What a fake ``sh`` returns: the real ``ShellOutput``'s awaited surface.

    ``next_value`` resolves to one value at a time and raises
    ``StopAsyncIteration`` when the command is done, which is the whole of what
    the gateway uses. Built from any iterable, so a test can hand in a list, a
    generator, or one that raises partway through.
    """

    def __init__(self, values: "Any") -> None:
        self._values = iter(values)

    async def next_value(self) -> "Any":
        try:
            return next(self._values)
        except StopIteration:
            raise StopAsyncIteration from None


class FakeConnection:
    """A node connection that records what it was asked to do.

    Implements the two methods the gateway calls on a ``Dusk`` instance: ``sh``
    to run a command and ``disconnect`` to hang up. ``sh`` echoes the command
    back as its single output value, so a test can tell which command reached
    which connection.

    ``produces`` replaces what ``sh`` hands back, for a test that needs a
    command to fail partway through or to return something JSON cannot carry.
    """

    def __init__(self, host: str, port: int) -> None:
        self.host = host
        self.port = port
        self.commands: list[str] = []
        self.disconnected = False
        self.produces: "Any" = None

    def sh(self, command: str) -> FakeOutput:
        self.commands.append(command)
        if self.produces is not None:
            return FakeOutput(self.produces)
        return FakeOutput([{"ran": command, "on": f"{self.host}:{self.port}"}])

    def disconnect(self) -> None:
        self.disconnected = True


class FakeConnectionFactory:
    """Opens :class:`FakeConnection` instances, and can be told to refuse.

    Set ``failure`` to an exception to make every later call raise it, which is
    how a test reproduces a node that will not accept a connection.
    """

    def __init__(self) -> None:
        self.connections: list[FakeConnection] = []
        self.failure: Exception | None = None

    def __call__(self, host: str, port: int) -> FakeConnection:
        if self.failure is not None:
            raise self.failure
        connection = FakeConnection(host, port)
        self.connections.append(connection)
        return connection


@pytest.fixture
def connection_factory() -> FakeConnectionFactory:
    return FakeConnectionFactory()


SERVED_AT = "127.0.0.1"
SERVED_ON = 9100
BASE_URL = f"http://{SERVED_AT}:{SERVED_ON}"
"""Where the gateway under test is pretending to be served.

The MCP endpoint checks the Host header against the address the gateway was told
it would be served on, so the requests a test sends have to agree with it. The
default TestClient base_url, ``http://testserver``, does not.
"""


@pytest.fixture
def gateway_app(connection_factory: FakeConnectionFactory):
    """The gateway application, wired to the fakes."""
    import dusk.gw

    return dusk.gw.app(
        SERVED_AT, connection_factory=connection_factory, programs=PROGRAMS
    )


@pytest.fixture
def client(gateway_app):
    """A test client with the app's lifespan running, so shutdown is exercised."""
    from starlette.testclient import TestClient

    with TestClient(gateway_app, base_url=BASE_URL) as test_client:
        yield test_client
