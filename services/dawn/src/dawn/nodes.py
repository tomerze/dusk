from __future__ import annotations

import asyncio
import functools
import logging
from collections.abc import Callable
from concurrent.futures import Future, ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol

from .config import Settings, split_host_port
from .models import NodeRef, pid_field

logger = logging.getLogger(__name__)

NODE_THREADS = 256
DISCONNECT_THREADS = 32
DISCONNECT_SECONDS = 10.0
CONNECTS = ThreadPoolExecutor(max_workers=NODE_THREADS, thread_name_prefix="dawn-node")
DISCONNECTS = ThreadPoolExecutor(
    max_workers=DISCONNECT_THREADS, thread_name_prefix="dawn-node-close"
)


class Output(Protocol):
    async def next_value(self) -> Any: ...


class Node(Protocol):
    def sh(self, command: str) -> Output: ...

    def disconnect(self) -> None: ...


@dataclass(frozen=True)
class NodeTarget:
    host: str
    port: int
    server_name: str
    ca: Path
    certificate: Path
    key: Path


Connector = Callable[[NodeTarget, int | None], Node]


class Unreachable(Exception):
    def __init__(self, message: str, late: Future[Node] | None = None) -> None:
        super().__init__(message)
        self.late = late


REFUSALS = (
    "Unimplemented: remote exception: not permitted: ",
    "Unimplemented: remote exception: unknown interface ",
    "Failed: remote exception: denied: not intended",
)


def denial(failure: BaseException) -> bool:
    return str(failure).startswith(REFUSALS)


class SessionsExhausted(Exception):
    pass


def described(failure: BaseException) -> str:
    return str(failure) or type(failure).__name__


def target_for(settings: Settings, node: NodeRef) -> NodeTarget:
    address = node.nightfall or settings.nightfall.default_inner_address
    host, port = split_host_port(address)
    return NodeTarget(
        host=host,
        port=port,
        server_name=f"{node.namespace_id}.{settings.nightfall.server_name_suffix}",
        ca=settings.nightfall.ca,
        certificate=settings.nightfall.certificate,
        key=settings.nightfall.key,
    )


class Sessions:
    def __init__(
        self, limit: int, changed: Callable[[int], None] = lambda count: None
    ) -> None:
        self._limit = limit
        self._open = 0
        self._changed = changed

    @property
    def open(self) -> int:
        return self._open

    def reserve(self) -> None:
        if self._open >= self._limit:
            raise SessionsExhausted(
                f"dawn holds {self._open} node sessions, its limit of limits.max_node_sessions"
            )
        self._open += 1
        self._changed(self._open)

    def release(self) -> None:
        self._open -= 1
        self._changed(self._open)


class Connection:
    def __init__(self, node: Node, fields: dict[str, Any]) -> None:
        self._node = node
        self.fields = fields
        self._closed = False

    def sh(self, command: str) -> Output:
        return self._node.sh(command)

    async def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        pending = asyncio.get_running_loop().run_in_executor(
            DISCONNECTS, _disconnect, self._node, self.fields
        )
        try:
            async with asyncio.timeout(DISCONNECT_SECONDS):
                await asyncio.shield(pending)
        except TimeoutError:
            logger.warning(
                "a node connection did not close in time; it goes on closing",
                extra={**self.fields, "seconds": DISCONNECT_SECONDS},
            )


def _disconnect(node: Node, fields: dict[str, Any]) -> None:
    try:
        node.disconnect()
    except Exception as failure:
        logger.warning(
            "a node connection did not close cleanly",
            extra={**fields, "error": described(failure)},
        )
        return
    logger.info("disconnected from a node", extra=fields)


def _abandon(fields: dict[str, Any], pending: Future[Node]) -> None:
    if pending.cancelled() or pending.exception() is not None:
        return
    logger.warning(
        "a node connection came up after dawn stopped waiting; closing it",
        extra=fields,
    )
    DISCONNECTS.submit(_disconnect, pending.result(), fields)


def _begin(began: asyncio.Future[None]) -> None:
    if not began.done():
        began.set_result(None)


async def connect(
    connector: Connector,
    settings: Settings,
    node: NodeRef,
    sh_server_pid: int | None,
) -> Connection:
    target = target_for(settings, node)
    fields: dict[str, Any] = {
        "device_id": node.device_id,
        "installation_id": node.installation_id,
        "namespace_id": node.namespace_id,
        "nightfall": f"{target.host}:{target.port}",
        "sh_server_pid": pid_field(sh_server_pid),
    }
    seconds = settings.nightfall.connect_timeout_seconds
    loop = asyncio.get_running_loop()
    began: asyncio.Future[None] = loop.create_future()

    def call() -> Node:
        loop.call_soon_threadsafe(_begin, began)
        return connector(target, sh_server_pid)

    pending = CONNECTS.submit(call)
    try:
        async with asyncio.timeout(seconds):
            await began
    except TimeoutError as failure:
        if pending.cancel():
            logger.warning(
                "no thread was free to connect to a node in time",
                extra={**fields, "seconds": seconds, "threads": NODE_THREADS},
            )
            raise SessionsExhausted(
                f"dawn is connecting to {NODE_THREADS} nodes already and none of them "
                f"finished within {seconds:g} s"
            ) from failure
    except asyncio.CancelledError:
        if not pending.cancel():
            pending.add_done_callback(functools.partial(_abandon, fields))
        raise
    try:
        async with asyncio.timeout(seconds):
            node_connection = await asyncio.shield(asyncio.wrap_future(pending))
    except TimeoutError as failure:
        pending.add_done_callback(functools.partial(_abandon, fields))
        logger.warning(
            "a node connection did not come up in time",
            extra={**fields, "seconds": seconds},
        )
        raise Unreachable(
            f"no connection to {target.server_name} through {target.host}:{target.port} "
            f"within {seconds:g} s",
            pending,
        ) from failure
    except asyncio.CancelledError:
        if not pending.cancel():
            pending.add_done_callback(functools.partial(_abandon, fields))
        raise
    except Exception as failure:
        logger.warning(
            "could not connect to a node",
            extra={**fields, "error": described(failure)},
        )
        if denial(failure):
            raise
        raise Unreachable(described(failure)) from failure
    logger.info("connected to a node", extra=fields)
    return Connection(node_connection, fields)
