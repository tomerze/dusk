from __future__ import annotations

import asyncio
import json
import logging
import time
from collections.abc import Awaitable, Callable
from typing import Any

import httpx
import jwt

logger = logging.getLogger(__name__)

MAX_DOCUMENT_BYTES = 1048576
MAX_KEYS = 64
REFRESH_ON_UNKNOWN_KEY_SECONDS = 30.0
RETRY_WITHOUT_KEYS_SECONDS = 5.0
FETCH_TIMEOUT_SECONDS = 5.0


class UnknownKey(LookupError):
    pass


def parse_key_set(document: Any, origin: str) -> dict[str | None, jwt.PyJWK]:
    if not isinstance(document, dict) or not isinstance(document.get("keys"), list):
        raise ValueError(f"{origin} is not a JWK set: no keys list")
    entries = document["keys"]
    if len(entries) > MAX_KEYS:
        raise ValueError(f"{origin} holds {len(entries)} keys, more than {MAX_KEYS}")
    keys: dict[str | None, jwt.PyJWK] = {}
    for entry in entries:
        if not isinstance(entry, dict) or entry.get("use", "sig") != "sig":
            continue
        try:
            key = jwt.PyJWK(entry)
        except jwt.PyJWTError as failure:
            logger.warning(
                "skipped a key the JWK set holds",
                extra={
                    "origin": origin,
                    "key_id": entry.get("kid"),
                    "error": str(failure),
                },
            )
            continue
        keys[entry.get("kid")] = key
    return keys


def select(keys: dict[str | None, jwt.PyJWK], key_id: str | None) -> jwt.PyJWK | None:
    if key_id in keys:
        return keys[key_id]
    if key_id is None and len(keys) == 1:
        return next(iter(keys.values()))
    return None


async def fetched(client: httpx.AsyncClient, location: str) -> Any:
    async with client.stream(
        "GET", location, timeout=FETCH_TIMEOUT_SECONDS
    ) as response:
        response.raise_for_status()
        body = bytearray()
        async for chunk in response.aiter_bytes():
            body += chunk
            if len(body) > MAX_DOCUMENT_BYTES:
                raise ValueError(
                    f"{location} answered more than {MAX_DOCUMENT_BYTES} bytes"
                )
    return json.loads(body)


class UrlKeys:
    def __init__(
        self,
        locate: Callable[[], Awaitable[str]],
        client: httpx.AsyncClient,
        cache_seconds: float,
        clock: Callable[[], float] = time.monotonic,
    ) -> None:
        self._locate = locate
        self._client = client
        self._cache_seconds = cache_seconds
        self._clock = clock
        self._keys: dict[str | None, jwt.PyJWK] = {}
        self._fetched_at: float | None = None
        self._attempted_at: float | None = None
        self._lock = asyncio.Lock()

    def _may_attempt(self) -> bool:
        if self._attempted_at is None:
            return True
        interval = (
            RETRY_WITHOUT_KEYS_SECONDS
            if not self._keys
            else REFRESH_ON_UNKNOWN_KEY_SECONDS
        )
        return self._clock() - self._attempted_at >= interval

    def _stale(self) -> bool:
        return (
            self._fetched_at is None
            or self._clock() - self._fetched_at >= self._cache_seconds
        )

    async def _refresh(self, reason: str, needed: Callable[[], bool]) -> None:
        async with self._lock:
            if not needed() or not self._may_attempt():
                return
            self._attempted_at = self._clock()
            location: str | None = None
            try:
                location = await self._locate()
                self._keys = parse_key_set(
                    await fetched(self._client, location), location
                )
            except (httpx.HTTPError, ValueError) as failure:
                logger.warning(
                    "could not fetch the JWK set",
                    extra={
                        "origin": location,
                        "reason": reason,
                        "error": str(failure),
                        "cached_keys": len(self._keys),
                    },
                )
                return
            self._fetched_at = self._clock()
            logger.info(
                "fetched a JWK set",
                extra={
                    "origin": location,
                    "reason": reason,
                    "key_count": len(self._keys),
                },
            )

    async def key(self, key_id: str | None) -> jwt.PyJWK:
        if self._stale():
            await self._refresh("expired", self._stale)
        key = select(self._keys, key_id)
        if key is None:
            await self._refresh(
                "unknown key", lambda: select(self._keys, key_id) is None
            )
            key = select(self._keys, key_id)
        if key is None:
            raise UnknownKey(f"no key {key_id!r} in the JWK set")
        return key
