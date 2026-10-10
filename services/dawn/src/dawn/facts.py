from __future__ import annotations

import json
import logging
import math
from collections.abc import AsyncIterator, Sequence
from typing import Any

from .nodes import Connection, Output

logger = logging.getLogger(__name__)

FACT_PREFIX = "dusk."
DEVICE_ID_KEY = "dusk.device.id"
CONFIG_HASH_KEY = "dusk.config.hash"
DEFAULT_VERSION_KEY = "dusk.version"
RUNNING_STATES = frozenset({"R", "RR"})
PS_NAME = "ps"
EVERY_FACT = "kvs get dusk."
NO_KEY_MATCHES = "no key matches"
MAX_READ_BYTES = 4 * 1024 * 1024
KVS_SALT = 0x93968E6E30A593D6
FNV_OFFSET_BASIS = 0xCBF29CE484222325
FNV_PRIME = 0x100000001B3
UNSIGNED_64 = 0xFFFFFFFFFFFFFFFF

Processes = dict[int, tuple[str, str]]


def finite(value: Any) -> Any:
    if isinstance(value, float) and not math.isfinite(value):
        return str(value)
    if isinstance(value, dict):
        return {name: finite(item) for name, item in value.items()}
    if isinstance(value, list | tuple):
        return [finite(item) for item in value]
    return value


def renderable(value: Any) -> Any:
    try:
        return json.loads(json.dumps(finite(value), default=str, allow_nan=False))
    except TypeError, ValueError:
        return repr(value)


def canonical(value: Any) -> bytes:
    return json.dumps(
        value,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
        allow_nan=False,
        default=str,
    ).encode()


async def values(output: Output) -> AsyncIterator[Any]:
    while True:
        try:
            value = await output.next_value()
        except StopAsyncIteration:
            return
        yield value


def record_fields(value: Any) -> dict[str, Any] | None:
    if not isinstance(value, dict) or len(value) != 1:
        return None
    ((type_id, fields),) = value.items()
    if (
        not isinstance(type_id, str)
        or not type_id.startswith("0x")
        or not isinstance(fields, dict)
    ):
        return None
    return fields


def key_id(name: str) -> int:
    identifier = FNV_OFFSET_BASIS ^ KVS_SALT
    for byte in name.encode():
        identifier = ((identifier ^ byte) * FNV_PRIME) & UNSIGNED_64
    return identifier


async def read(
    connection: Connection, statements: Sequence[str] = (), names: Sequence[str] = ()
) -> tuple[dict[str, Any], Processes]:
    rows, processes = await answered(connection, "; ".join([*statements, "ps"]))
    for name in names:
        if row_name(rows, name) is not None:
            continue
        try:
            found, _ = await answered(connection, f"kvs get {name}")
        except RuntimeError as failure:
            if NO_KEY_MATCHES not in str(failure):
                raise
            continue
        rows.update(found)
    return rows, processes


async def answered(
    connection: Connection, command: str
) -> tuple[dict[str, Any], Processes]:
    rows: dict[str, Any] = {}
    processes: Processes = {}
    size = 0
    async for value in values(connection.sh(command)):
        rendered = renderable(value)
        size += len(canonical(rendered))
        if size > MAX_READ_BYTES:
            raise ValueError(
                f"the node answered {command!r} with more than {MAX_READ_BYTES} bytes"
            )
        fields = record_fields(rendered)
        if fields is None:
            continue
        if "PID" in fields:
            processes.update(process_rows(fields))
        elif "Key" in fields and "Value" in fields:
            rows.update(key_rows(fields))
    return rows, processes


def key_rows(fields: dict[str, Any]) -> dict[str, Any]:
    names, found = fields["Key"], fields["Value"]
    if not isinstance(names, list) or not isinstance(found, list):
        logger.warning(
            "kvs answered rows that are not lists; they are left out",
            extra={"fields": sorted(fields)},
        )
        return {}
    return {
        name: value
        for name, value in zip(names, found, strict=False)
        if isinstance(name, str)
    }


def process_rows(fields: dict[str, Any]) -> Processes:
    pids, names, states = fields.get("PID"), fields.get("Name"), fields.get("State")
    if (
        not isinstance(pids, list)
        or not isinstance(names, list)
        or not isinstance(states, list)
    ):
        logger.warning(
            "ps answered rows that are not lists; they are left out",
            extra={"fields": sorted(fields)},
        )
        return {}
    return {
        pid: (str(name), str(state))
        for pid, name, state in zip(pids, names, states, strict=False)
        if isinstance(pid, int) and not isinstance(pid, bool)
    }


def row_name(rows: dict[str, Any], key: str) -> str | None:
    for name in (key, f"{key_id(key):#018x}"):
        if name in rows:
            return name
    return None


def text(value: Any) -> str | None:
    return value if isinstance(value, str) or value is None else json.dumps(value)


def value_of(rows: dict[str, Any], key: str) -> str | None:
    name = row_name(rows, key)
    return text(rows[name]) if name is not None else None


def services(processes: Processes) -> list[str]:
    return sorted(
        {
            name
            for name, state in processes.values()
            if state in RUNNING_STATES and name != PS_NAME
        }
    )


def reported(
    rows: dict[str, Any],
    processes: Processes,
    version_key: str | None,
    found: dict[str, Any] | None = None,
) -> dict[str, Any]:
    return {
        "version_key": version_key,
        "version": value_of(rows, version_key) if version_key is not None else None,
        "config_hash": value_of(rows, CONFIG_HASH_KEY),
        "services": services(processes),
        "facts": found,
    }


async def collect(
    connection: Connection, version_keys: Sequence[str]
) -> tuple[dict[str, Any], dict[str, Any]]:
    exact = list(dict.fromkeys([CONFIG_HASH_KEY, *version_keys]))
    rows, processes = await read(connection, [EVERY_FACT], exact)
    found = {
        name: value for name, value in rows.items() if name.startswith(FACT_PREFIX)
    }
    for key in exact:
        name = row_name(rows, key)
        if name is not None:
            found[key] = rows[name]
    found.pop(DEVICE_ID_KEY, None)
    version_key = version_keys[0] if version_keys else DEFAULT_VERSION_KEY
    return found, reported(rows, processes, version_key, found)
