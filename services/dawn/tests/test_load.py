from __future__ import annotations

import asyncio
import json
import os
import time
from pathlib import Path
from typing import Any

import pytest

from dawn import facts
from dawn.config import split_host_port
from dawn.file_limit import raise_file_limit
from dawn.nodes import CONNECTS, DISCONNECTS, NODE_THREADS

pytestmark = [pytest.mark.load, pytest.mark.anyio]

SESSIONS = int(os.environ.get("DAWN_LOAD_SESSIONS", "10000"))
HOLD_SECONDS = float(os.environ.get("DAWN_LOAD_HOLD_SECONDS", "30"))
SPARE_THREADS = 32
CONCURRENT_CONNECTS = 32
SAMPLE = 16
TLS_VARIABLES = {
    "server_name": "DAWN_LOAD_SERVER_NAME",
    "ca": "DAWN_LOAD_CA",
    "certificate": "DAWN_LOAD_CERTIFICATE",
    "key": "DAWN_LOAD_KEY",
}


def process_status() -> dict[str, int]:
    fields: dict[str, int] = {}
    for line in Path("/proc/self/status").read_text().splitlines():
        name, _, value = line.partition(":")
        if name in ("Threads", "VmRSS"):
            fields[name] = int(value.split()[0])
    return {"threads": fields["Threads"], "rss_kib": fields["VmRSS"]}


@pytest.fixture
def address() -> tuple[str, int]:
    configured = os.environ.get("DAWN_LOAD_NODE")
    if not configured:
        pytest.skip(
            "set DAWN_LOAD_NODE=host:port of a dusk node, or of nightfall's inner "
            "listener together with " + ", ".join(TLS_VARIABLES.values())
        )
    return split_host_port(configured)


def connection_arguments() -> dict[str, Any]:
    if not os.environ.get("DAWN_LOAD_CA"):
        return {}
    return {name: os.environ[variable] for name, variable in TLS_VARIABLES.items()}


async def test_one_dawn_process_holds_ten_thousand_idle_node_sessions(
    address: tuple[str, int],
):
    import dusk

    host, port = address
    raise_file_limit(SESSIONS)
    arguments = connection_arguments()
    loop = asyncio.get_running_loop()

    slots = asyncio.Semaphore(CONCURRENT_CONNECTS)

    async def opened() -> Any:
        async with slots:
            return await loop.run_in_executor(
                CONNECTS, lambda: dusk.Dusk(host, port, **arguments)
            )

    before = process_status()
    began = time.monotonic()
    connections = await asyncio.gather(*(opened() for _ in range(SESSIONS)))
    connect_seconds = time.monotonic() - began
    try:
        await asyncio.sleep(HOLD_SECONDS)
        held = process_status()
        for connection in connections[:: max(1, SESSIONS // SAMPLE)]:
            answered = [value async for value in facts.values(connection.sh("ps"))]
            assert answered, "a held session did not answer ps"
        report = {
            "sessions": SESSIONS,
            "connect_seconds": round(connect_seconds, 3),
            "hold_seconds": HOLD_SECONDS,
            "threads_before": before["threads"],
            "threads_held": held["threads"],
            "rss_kib_before": before["rss_kib"],
            "rss_kib_held": held["rss_kib"],
            "rss_kib_per_session": round(
                (held["rss_kib"] - before["rss_kib"]) / SESSIONS, 3
            ),
        }
        print(json.dumps(report))
        destination = os.environ.get("DAWN_LOAD_REPORT")
        if destination:
            Path(destination).write_text(json.dumps(report, indent=2) + "\n")
        workers = os.process_cpu_count() or 1
        assert (
            held["threads"] - before["threads"]
            <= workers + NODE_THREADS + SPARE_THREADS
        ), report
    finally:
        await asyncio.gather(
            *(
                loop.run_in_executor(DISCONNECTS, connection.disconnect)
                for connection in connections
            )
        )
