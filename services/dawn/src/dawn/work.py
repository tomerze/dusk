from __future__ import annotations

import asyncio
import contextlib
import logging
from collections.abc import AsyncIterator, Sequence
from typing import Any

from . import facts
from .config import Settings
from .models import NodeRef, pid_field
from .nodes import Connection, Connector, connect, described

logger = logging.getLogger(__name__)

REAP_ROUNDS = 3
REAP_WAIT_SECONDS = 0.25
RELEASE_SECONDS = 120.0


async def release(
    connector: Connector,
    settings: Settings,
    node: NodeRef,
    pids: Sequence[int],
    log: dict[str, Any],
) -> list[int]:
    fields = {**log, "released_pids": [pid_field(pid) for pid in pids]}
    try:
        async with asyncio.timeout(RELEASE_SECONDS):
            shell = await connect(connector, settings, node, None)
            try:
                left = await reap_pids(shell, list(pids))
            finally:
                await shell.close()
    except asyncio.CancelledError:
        logger.warning(
            "dawn stopped before killing and reaping processes on a node",
            extra=fields,
        )
        raise
    except Exception as failure:
        logger.warning(
            "could not kill and reap processes on a node",
            extra={**fields, "error": described(failure)},
        )
        return list(pids)
    if left:
        logger.warning(
            "some processes did not exit and were not reaped",
            extra={**fields, "left_pids": [pid_field(pid) for pid in left]},
        )
    else:
        logger.info("killed and reaped processes on a node", extra=fields)
    return left


@contextlib.asynccontextmanager
async def shell_at(
    connector: Connector,
    settings: Settings,
    node: NodeRef,
    pid: int,
    log: dict[str, Any],
) -> AsyncIterator[Connection]:
    connection = await connect(connector, settings, node, pid)
    try:
        yield connection
    finally:
        await connection.close()
        await release(connector, settings, node, [pid], log)


async def reap_pids(connection: Connection, pids: list[int]) -> list[int]:
    _, processes = await facts.read(connection, [f"kill {pid:#x}" for pid in pids])
    for round_index in range(REAP_ROUNDS):
        left = [pid for pid in pids if pid in processes]
        exited = [pid for pid in left if processes[pid][1] == "Z"]
        if not left:
            return []
        if not exited:
            await asyncio.sleep(REAP_WAIT_SECONDS * 2**round_index)
        _, processes = await facts.read(
            connection, [f"kill --signal 8 {pid:#x}" for pid in exited]
        )
    return [pid for pid in pids if pid in processes]
