from __future__ import annotations

import asyncio
import contextlib
import hashlib
import hmac
import logging
import time
from collections.abc import AsyncIterator, Awaitable, Callable, Sequence
from dataclasses import dataclass, field
from typing import Any, Protocol

from opentelemetry import trace

from . import facts
from .config import Settings
from .events import MAX_MESSAGE_BYTES, Events, timestamp
from .models import NodeRef, Work, pid_field
from .nodes import Connection, Connector, Unreachable, connect, denial, described

logger = logging.getLogger(__name__)
tracer = trace.get_tracer("dawn")

KIND_ORDER = {"quarantine": 0, "run_script": 1, "ensure_config": 2, "ensure_version": 3}
TRANSPORT_FAILURES = ("disconnected", "overloaded", "connection is closed")
ENDED = frozenset({"succeeded", "failed"})
OVER = ENDED | {"ended"}
OUTPUT_FLUSH_BYTES = 262144
FINAL_SEND_SECONDS = 10.0
STOP_SECONDS = 10.0
REAP_ROUNDS = 3
REAP_WAIT_SECONDS = 0.25
RELEASE_SECONDS = 120.0
SCRIPT_SPAN = "sh_exec"
SCRIPT_LOGS = "logs dump --replay-only -l info"


def ordered(work: list[tuple[NodeRef, Work]]) -> list[tuple[NodeRef, Work]]:
    return sorted(work, key=lambda queued: KIND_ORDER[queued[1].kind])


def transport(failure: BaseException) -> bool:
    return str(failure).lower().startswith(TRANSPORT_FAILURES)


def classify(failure: BaseException, delivered: bool) -> str:
    if isinstance(failure, Unreachable):
        return "unreachable"
    if isinstance(failure, TimeoutError):
        return "timed_out"
    if denial(failure):
        return "denied"
    if not delivered and transport(failure):
        return "unreachable"
    return "error"


def log_attributes(log: dict[str, Any]) -> dict[str, str | int]:
    return {
        f"dawn.{name}": value
        for name, value in log.items()
        if isinstance(value, (str, int)) and not isinstance(value, bool)
    }


class Collected:
    def __init__(
        self,
        key: bytes,
        max_bytes: int,
        emit: Callable[[int, Any, bool], Awaitable[Awaitable[bool]]],
    ) -> None:
        self._digest = hmac.new(key, digestmod=hashlib.sha256)
        self._max_bytes = max_bytes
        self._emit = emit
        self._kept_bytes = 0
        self._unflushed_bytes = 0
        self._kept = 0
        self._pending: list[Awaitable[bool]] = []
        self.count = 0
        self.truncated = False
        self.lost = False

    @property
    def digest(self) -> str:
        return self._digest.hexdigest()

    async def add(self, value: Any) -> None:
        rendered = facts.renderable(value)
        encoded = facts.canonical(rendered)
        self._digest.update(encoded + b"\n")
        self.count += 1
        if self.truncated:
            return
        if (
            self._kept_bytes + len(encoded) > self._max_bytes
            or len(encoded) > MAX_MESSAGE_BYTES - 4096
        ):
            self.truncated = True
            self._pending.append(await self._emit(self._kept, None, True))
            return
        self._pending.append(await self._emit(self._kept, rendered, False))
        self._kept += 1
        self._kept_bytes += len(encoded)
        self._unflushed_bytes += len(encoded)
        if self._unflushed_bytes >= OUTPUT_FLUSH_BYTES:
            await self.flush()

    async def flush(self) -> None:
        pending, self._pending = self._pending, []
        self._unflushed_bytes = 0
        for delivery in pending:
            if not await delivery:
                self.lost = True


@dataclass
class Attempt:
    node: NodeRef
    pid: int
    campaign_id: str | None
    attempt: int | None
    action_kind: str
    started_at: str = field(default_factory=timestamp)
    status: str = "error"
    known: bool = False
    delivered: bool = False
    error: str | None = None
    output_digest: str = ""
    output_count: int = 0
    output_truncated: bool = False
    reported: dict[str, Any] | None = None

    def fields(self, finished: bool) -> dict[str, Any]:
        return {
            "pid": str(self.pid),
            "campaign_id": self.campaign_id,
            "attempt": self.attempt,
            "device_id": self.node.device_id,
            "installation_id": self.node.installation_id,
            "namespace_id": self.node.namespace_id,
            "action_kind": self.action_kind,
            "status": self.status if finished else "started",
            "delivered": self.delivered,
            "error": self.error,
            "started_at": self.started_at,
            "finished_at": timestamp() if finished else None,
            "output_digest": self.output_digest,
            "output_count": self.output_count,
            "output_truncated": self.output_truncated,
            "reported": self.reported,
        }

    def log(self) -> dict[str, Any]:
        return {
            "pid": pid_field(self.pid),
            "campaign_id": self.campaign_id,
            "attempt": self.attempt,
            "device_id": self.node.device_id,
            "installation_id": self.node.installation_id,
            "namespace_id": self.node.namespace_id,
            "action_kind": self.action_kind,
        }


class Results:
    def __init__(
        self,
        events: Events,
        output_key: bytes,
        counted: Callable[[str, str], None] = lambda action_kind, status: None,
    ) -> None:
        self.events = events
        self.output_key = output_key
        self._counted = counted

    def attempt(
        self,
        node: NodeRef,
        pid: int,
        campaign_id: str | None,
        attempt: int | None,
        action_kind: str,
    ) -> Attempt:
        made = Attempt(node, pid, campaign_id, attempt, action_kind)
        made.output_digest = hmac.new(
            self.output_key, digestmod=hashlib.sha256
        ).hexdigest()
        return made

    async def started(self, attempt: Attempt) -> None:
        await self.events.process_result(**attempt.fields(finished=False))

    async def finished(self, attempt: Attempt) -> None:
        self._counted(attempt.action_kind, attempt.status)
        await self.events.process_result(**attempt.fields(finished=True))

    async def finished_despite_cancellation(self, attempt: Attempt) -> None:
        sending = asyncio.ensure_future(self.finished(attempt))
        try:
            async with asyncio.timeout(FINAL_SEND_SECONDS):
                await asyncio.shield(sending)
        except (TimeoutError, asyncio.CancelledError) as failure:
            sending.cancel()
            logger.error(
                "could not report work dawn stopped",
                extra={**attempt.log(), "error": described(failure)},
            )


class FileCollector(Protocol):
    async def collect(
        self,
        connection: Connection,
        attempt: Attempt,
        path: str,
        index: int,
        deadline: float,
    ) -> None: ...


class Runner:
    def __init__(
        self,
        settings: Settings,
        connector: Connector,
        results: Results,
        files: FileCollector | None = None,
        clock: Callable[[], float] = time.monotonic,
    ) -> None:
        self._settings = settings
        self._connector = connector
        self._results = results
        self._files = files
        self._clock = clock

    def timeout(self, work: Work) -> int:
        return min(work.timeout_seconds, self._settings.limits.process_timeout_max)

    def remaining(self, deadline: float) -> float:
        return max(0.0, deadline - self._clock())

    def attempt(self, node: NodeRef, work: Work, action_kind: str) -> Attempt:
        return self._results.attempt(
            node, int(work.pid), work.campaign_id, work.attempt, action_kind
        )

    async def unreachable(
        self,
        node: NodeRef,
        work: Work,
        failure: BaseException,
        despite_cancellation: bool = False,
    ) -> None:
        attempt = self.attempt(node, work, work.kind)
        attempt.status = classify(failure, delivered=False)
        attempt.error = described(failure)
        if despite_cancellation:
            await self._results.finished_despite_cancellation(attempt)
        else:
            await self._results.finished(attempt)

    async def run(
        self,
        node: NodeRef,
        work: Work,
        finishing: Callable[[], None] = lambda: None,
    ) -> str:
        attempt = self.attempt(node, work, work.kind)
        deadline = self._clock() + self.timeout(work)
        log = attempt.log()
        logger.info("running work on a node", extra=log)
        with tracer.start_as_current_span(
            "work", attributes=log_attributes(log)
        ) as span:
            await self._traced_run(node, work, attempt, deadline, log, finishing)
            span.set_attribute("dawn.status", attempt.status)
            return attempt.status

    async def _traced_run(
        self,
        node: NodeRef,
        work: Work,
        attempt: Attempt,
        deadline: float,
        log: dict[str, Any],
        finishing: Callable[[], None],
    ) -> None:
        connections: list[Connection] = []
        try:
            try:
                async with asyncio.timeout(self.remaining(deadline)):
                    default = await connect(self._connector, self._settings, node, None)
                    connections.append(default)
                    await self._deliver(default, node, work, attempt, connections)
                    if attempt.status in ENDED:
                        await self._after(
                            default, connections[-1], work, attempt, deadline, log
                        )
            except asyncio.CancelledError:
                raise
            except Exception as failure:
                if attempt.known:
                    message = "the steps after the work did not finish"
                else:
                    message = "work did not finish"
                    attempt.status = classify(failure, attempt.delivered)
                    attempt.error = described(failure)
                    attempt.known = True
                logger.warning(
                    message,
                    extra={
                        **log,
                        "status": attempt.status,
                        "error": described(failure),
                    },
                )
            await self._stop(connections, attempt, log)
            finishing()
            await self._results.finished(attempt)
            logger.info(
                "finished work on a node",
                extra={**log, "status": attempt.status, "delivered": attempt.delivered},
            )
        except asyncio.CancelledError:
            if attempt.known:
                logger.warning(
                    "dawn stopped the steps after the work; its result stands",
                    extra={**log, "status": attempt.status},
                )
            elif attempt.delivered:
                attempt.error = "dawn stopped before the script ended"
                attempt.status = "running"
                try:
                    async with asyncio.timeout(FINAL_SEND_SECONDS):
                        if await self._script_ended(connections[0], attempt):
                            attempt.status = "ended"
                except TimeoutError:
                    logger.warning(
                        "could not read the node's logs before dawn stopped",
                        extra=log,
                    )
                attempt.known = True
                logger.warning(
                    "dawn stopped while the script ran",
                    extra={**log, "status": attempt.status},
                )
            else:
                attempt.status = "error"
                attempt.error = "dawn stopped before the work finished"
            await self._stop(connections, attempt, log)
            finishing()
            await self._results.finished_despite_cancellation(attempt)
            raise
        finally:
            for connection in connections:
                await connection.close()

    async def _deliver(
        self,
        default: Connection,
        node: NodeRef,
        work: Work,
        attempt: Attempt,
        connections: list[Connection],
    ) -> None:
        checked = checked_key(work)
        names = [] if checked is None else reported_names(work)
        rows, processes = await facts.read(default, (), names)
        if checked is not None:
            attempt.reported = facts.reported(rows, processes, work.version_key)
        if attempt.pid in processes:
            attempt.status = "duplicate"
            attempt.known = True
            logger.info(
                "the work's process is on the node already; nothing runs again",
                extra={**attempt.log(), "state": processes[attempt.pid][1]},
            )
            return
        if checked is not None and facts.value_of(rows, checked) == desired_value(work):
            attempt.status = "already_satisfied"
            attempt.known = True
            return
        try:
            shell = await connect(self._connector, self._settings, node, attempt.pid)
        except Exception as failure:
            await self._withdraw(default, attempt, failure)
            raise
        connections.append(shell)
        attempt.delivered = True
        await self._results.started(attempt)
        await self._script(default, shell, work, attempt)

    async def _script(
        self, default: Connection, shell: Connection, work: Work, attempt: Attempt
    ) -> None:
        assert work.script is not None
        node = attempt.node

        async def emit(index: int, value: Any, truncated: bool) -> Awaitable[bool]:
            return await self._results.events.process_output(
                pid=str(attempt.pid),
                campaign_id=attempt.campaign_id,
                device_id=node.device_id,
                installation_id=node.installation_id,
                namespace_id=node.namespace_id,
                index=index,
                value=value,
                truncated=truncated,
            )

        collected = Collected(
            self._results.output_key, self._settings.limits.max_output_bytes, emit
        )
        try:
            async for value in facts.values(shell.sh(work.script)):
                await collected.add(value)
            attempt.status = "succeeded"
        except RuntimeError as failure:
            if denial(failure):
                raise
            attempt.error = described(failure)
            if not transport(failure):
                attempt.status = "failed"
            elif await self._script_ended(default, attempt):
                attempt.status = "ended"
            else:
                attempt.status = "running"
        finally:
            await collected.flush()
            attempt.output_digest = collected.digest
            attempt.output_count = collected.count
            attempt.output_truncated = collected.truncated or collected.lost
        attempt.known = True

    async def _withdraw(
        self, default: Connection, attempt: Attempt, failure: Exception
    ) -> None:
        late = failure.late if isinstance(failure, Unreachable) else None
        if late is not None:
            with contextlib.suppress(Exception):
                await asyncio.wrap_future(late)
        try:
            left = await reap_pids(default, [attempt.pid])
        except (RuntimeError, ValueError) as unread:
            logger.warning(
                "could not check the work's pid after its connection failed",
                extra={**attempt.log(), "error": described(unread)},
            )
            return
        if left:
            attempt.delivered = True
            raise RuntimeError(
                "a shell server stayed at the pid after its connection failed: "
                f"{described(failure)}"
            ) from failure

    async def _script_ended(self, default: Connection, attempt: Attempt) -> bool:
        wanted = pid_field(attempt.pid)
        try:
            async for value in facts.values(default.sh(SCRIPT_LOGS)):
                fields = facts.record_fields(value)
                if (
                    fields is not None
                    and fields.get("name") == SCRIPT_SPAN
                    and "endTimeUnixNano" in fields
                    and (facts.record_fields(fields.get("attributes")) or {}).get("pid")
                    == wanted
                ):
                    return True
        except RuntimeError as failure:
            logger.warning(
                "could not read the node's logs after the stream broke",
                extra={**attempt.log(), "error": described(failure)},
            )
        return False

    async def _after(
        self,
        default: Connection,
        shell: Connection,
        work: Work,
        attempt: Attempt,
        deadline: float,
        log: dict[str, Any],
    ) -> None:
        try:
            if work.collect_facts:
                version_keys = [work.version_key] if work.version_key else []
                _, attempt.reported = await facts.collect(default, version_keys)
            elif checked_key(work) is not None:
                rows, processes = await facts.read(default, (), reported_names(work))
                attempt.reported = facts.reported(rows, processes, work.version_key)
        except (RuntimeError, ValueError) as failure:
            logger.warning(
                "could not read the node's state after the work",
                extra={**log, "error": described(failure)},
            )
        if self._files is not None:
            for index, path in enumerate(work.collect_files):
                await self._files.collect(
                    shell,
                    self.attempt(attempt.node, work, "collect_file"),
                    path,
                    index,
                    deadline,
                )
        if work.stream_logs is not None:
            seconds = min(
                work.stream_logs.duration_seconds,
                self._settings.limits.max_log_stream_seconds,
                self.remaining(deadline),
            )
            await stream_logs(
                shell,
                self._settings.collector.endpoint,
                work.stream_logs.level,
                seconds,
                log,
            )

    async def _stop(
        self, connections: list[Connection], attempt: Attempt, log: dict[str, Any]
    ) -> None:
        if not attempt.delivered or attempt.status not in OVER:
            return
        try:
            async with asyncio.timeout(STOP_SECONDS):
                await facts.read(connections[0], [f"kill {attempt.pid:#x}"])
        except (RuntimeError, ValueError, TimeoutError) as failure:
            logger.warning(
                "could not stop the work's shell server; it runs until it is reaped",
                extra={**log, "error": described(failure)},
            )

    async def reap(self, node: NodeRef, pids: Sequence[int]) -> None:
        attempts = [
            self._results.attempt(node, pid, None, None, "reap") for pid in pids
        ]
        log = {
            "device_id": node.device_id,
            "installation_id": node.installation_id,
            "namespace_id": node.namespace_id,
            "reaped_pids": [pid_field(pid) for pid in pids],
        }
        try:
            async with asyncio.timeout(RELEASE_SECONDS):
                default = await connect(self._connector, self._settings, node, None)
                try:
                    left = set(await reap_pids(default, list(pids)))
                finally:
                    await default.close()
            for attempt in attempts:
                if attempt.pid in left:
                    attempt.error = "the process did not exit"
                else:
                    attempt.status = "reaped"
        except asyncio.CancelledError:
            for attempt in attempts:
                attempt.error = "dawn stopped before reaping the process"
                await self._results.finished_despite_cancellation(attempt)
            raise
        except Exception as failure:
            logger.warning(
                "could not reap processes on a node",
                extra={**log, "error": described(failure)},
            )
            for attempt in attempts:
                attempt.status = classify(failure, delivered=False)
                attempt.error = described(failure)
        for attempt in attempts:
            await self._results.finished(attempt)


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


def checked_key(work: Work) -> str | None:
    if work.kind == "ensure_version":
        return work.version_key
    if work.kind == "ensure_config":
        return facts.CONFIG_HASH_KEY
    return None


def reported_names(work: Work) -> list[str]:
    return [
        name for name in (work.version_key, facts.CONFIG_HASH_KEY) if name is not None
    ]


def desired_value(work: Work) -> str | None:
    if work.kind == "ensure_version":
        return work.desired_version
    return work.config_hash


async def stream_logs(
    connection: Connection,
    endpoint: str,
    level: str,
    seconds: float,
    log: dict[str, Any],
) -> Exception | None:
    command = f"logs stream otlp://{endpoint} -l {level}"
    try:
        output = connection.sh(command)
        async with asyncio.timeout(seconds):
            async for _ in facts.values(output):
                pass
    except TimeoutError:
        logger.info(
            "streamed a node's logs for their whole duration",
            extra={**log, "endpoint": endpoint},
        )
        return None
    except asyncio.CancelledError:
        raise
    except Exception as failure:
        logger.warning(
            "a node's log stream failed",
            extra={**log, "endpoint": endpoint, "error": described(failure)},
        )
        return failure
    logger.info(
        "a node's log stream ended before its duration",
        extra={**log, "endpoint": endpoint},
    )
    return None
