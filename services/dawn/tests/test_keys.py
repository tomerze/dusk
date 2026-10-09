from __future__ import annotations

import json

import httpx
import pytest
from cryptography.hazmat.primitives.asymmetric import ec
from jwt.algorithms import ECAlgorithm

from dawn.keys import UnknownKey, UrlKeys

pytestmark = pytest.mark.anyio

LOCATION = "https://id.example.org/realms/dusk/protocol/openid-connect/certs"


def key_entry(key_id: str) -> dict:
    private_key = ec.generate_private_key(ec.SECP256R1())
    entry = json.loads(ECAlgorithm.to_jwk(private_key.public_key()))
    entry.update({"kid": key_id, "use": "sig", "alg": "ES256"})
    return entry


class Clock:
    def __init__(self) -> None:
        self.now = 1000.0

    def __call__(self) -> float:
        return self.now


class KeyServer:
    def __init__(self, *key_ids: str) -> None:
        self.entries = [key_entry(key_id) for key_id in key_ids]
        self.requests = 0
        self.failing = False

    def handle(self, request: httpx.Request) -> httpx.Response:
        self.requests += 1
        if self.failing:
            return httpx.Response(503)
        return httpx.Response(200, json={"keys": self.entries})


async def located() -> str:
    return LOCATION


def url_keys(server: KeyServer, clock: Clock, cache_seconds: float = 300) -> UrlKeys:
    client = httpx.AsyncClient(transport=httpx.MockTransport(server.handle))
    return UrlKeys(located, client, cache_seconds, clock)


async def test_a_key_set_url_is_fetched_once_and_cached():
    server = KeyServer("issuer-1")
    clock = Clock()
    keys = url_keys(server, clock)

    await keys.key("issuer-1")
    clock.now += 100
    await keys.key("issuer-1")

    assert server.requests == 1


async def test_a_key_set_url_is_fetched_again_once_its_cache_expires():
    server = KeyServer("issuer-1")
    clock = Clock()
    keys = url_keys(server, clock, cache_seconds=60)

    await keys.key("issuer-1")
    clock.now += 61
    await keys.key("issuer-1")

    assert server.requests == 2


async def test_an_unknown_key_fetches_the_set_again_but_not_on_every_request():
    server = KeyServer("issuer-1")
    clock = Clock()
    keys = url_keys(server, clock)
    await keys.key("issuer-1")

    for _ in range(2):
        with pytest.raises(UnknownKey):
            await keys.key("issuer-2")
    assert server.requests == 1

    clock.now += 31
    server.entries.append(key_entry("issuer-2"))

    assert await keys.key("issuer-2")
    assert server.requests == 2


async def test_a_key_without_an_id_is_the_only_key_of_the_set():
    keys = url_keys(KeyServer("issuer-1"), Clock())

    assert await keys.key(None) == await keys.key("issuer-1")


async def test_a_failing_key_set_url_keeps_the_keys_fetched_before():
    server = KeyServer("issuer-1")
    clock = Clock()
    keys = url_keys(server, clock, cache_seconds=60)
    await keys.key("issuer-1")

    server.failing = True
    clock.now += 61

    assert await keys.key("issuer-1")
    assert server.requests == 2


async def test_a_key_set_url_that_never_answered_knows_no_key():
    server = KeyServer("issuer-1")
    server.failing = True

    with pytest.raises(UnknownKey):
        await url_keys(server, Clock()).key("issuer-1")


async def test_a_key_set_url_answering_too_much_is_not_read_to_its_end():
    sent: list[int] = []

    async def endless():
        for index in range(64):
            sent.append(index)
            yield b" " * 65536

    def handle(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, content=endless())

    client = httpx.AsyncClient(transport=httpx.MockTransport(handle))
    keys = UrlKeys(located, client, 300, Clock())

    with pytest.raises(UnknownKey):
        await keys.key("issuer-1")
    assert len(sent) < 64
