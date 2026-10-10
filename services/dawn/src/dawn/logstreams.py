from __future__ import annotations

import asyncio
import logging
import time
import uuid
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

from .config import Settings
from .events import timestamp
from .models import LogStream, LogStreamRequest, pid_field
from .nodes import Connection, Connector, Sessions, described
from .work import Results, classify, shell_at, stream_logs

logger = logging.getLogger(__name__)


class StreamsExhausted(Exception):
    pass


@dataclass
class ActiveStream:
    description: LogStream
    request: LogStreamRequest
    endpoint: str
    seconds: float
    stopped: asyncio.Event = field(default_factory=asyncio.Event)
    task: asyncio.Task[None] | None = None


class LogStreams:
    def __init__(
        self,
        settings: Settings,
        connector: Connector,
        sessions: Sessions,
        results: Results,
        changed: Callable[[int], None] = lambda count: None,
        clock: Callable[[], float] = time.time,
    ) -> None:
        self._settings = settings
        self._connector = connector
        self._sessions = sessions
        self._results = results
        self._changed = changed
        self._clock = clock
        self._streams: dict[str, ActiveStream] = {}

    def list(self) -> list[LogStream]:
        return [stream.description for stream in self._streams.values()]

    def start(self, request: LogStreamRequest, principal: str) -> str:
        for running in self._streams.values():
            if running.request == request:
                return running.description.stream_id
        limit = self._settings.limits.max_log_streams
        if len(self._streams) >= limit:
            raise StreamsExhausted(
                f"dawn already runs {limit} log streams (limits.max_log_streams)"
            )
        self._sessions.reserve()
        endpoint = request.endpoint or self._settings.collector.endpoint
        now = self._clock()
        stream_id = str(uuid.uuid4())
        stream = ActiveStream(
            description=LogStream(
                stream_id=stream_id,
                device_id=request.node.device_id,
                installation_id=request.node.installation_id,
                namespace_id=request.node.namespace_id,
                pid=request.pid,
                level=request.level,
                endpoint=endpoint,
                started_at=timestamp(int(now * 1_000_000_000)),
                ends_at=timestamp(
                    int((now + request.duration_seconds) * 1_000_000_000)
                ),
                principal=principal,
            ),
            request=request,
            endpoint=endpoint,
            seconds=request.duration_seconds,
        )
        self._streams[stream_id] = stream
        self._changed(len(self._streams))
        stream.task = asyncio.get_running_loop().create_task(self._run(stream))
        logger.info(
            "started a log stream",
            extra={
                "stream_id": stream_id,
                "pid": pid_field(request.pid),
                "device_id": request.node.device_id,
                "installation_id": request.node.installation_id,
                "namespace_id": request.node.namespace_id,
                "endpoint": endpoint,
                "level": request.level,
                "duration_seconds": request.duration_seconds,
                "principal": principal,
            },
        )
        return stream_id

    def get(self, stream_id: str) -> LogStream | None:
        stream = self._streams.get(stream_id)
        return stream.description if stream is not None else None

    async def stop(self, stream_id: str) -> bool:
        stream = self._streams.get(stream_id)
        if stream is None:
            return False
        stream.stopped.set()
        if stream.task is not None:
            await asyncio.wait({stream.task})
        return True

    async def _run(self, stream: ActiveStream) -> None:
        request = stream.request
        node = request.node
        attempt = self._results.attempt(
            node, int(request.pid), None, None, "stream_logs"
        )
        log = {**attempt.log(), "stream_id": stream.description.stream_id}
        failure: Exception | None = None
        try:
            try:
                async with shell_at(
                    self._connector, self._settings, node, attempt.pid, log
                ) as connection:
                    attempt.delivered = True
                    failure = await self._stream(connection, stream, log)
                    attempt.status = "succeeded" if failure is None else "error"
                    attempt.known = True
            except asyncio.CancelledError:
                if not attempt.known:
                    attempt.status = "error"
                    attempt.error = "dawn stopped the log stream"
                await self._results.finished_despite_cancellation(attempt)
                raise
            except Exception as unreached:
                failure = unreached
            if failure is not None:
                attempt.status = classify(failure, attempt.delivered)
                attempt.error = described(failure)
            await self._results.finished(attempt)
        finally:
            del self._streams[stream.description.stream_id]
            self._changed(len(self._streams))
            self._sessions.release()
            logger.info("ended a log stream", extra={**log, "status": attempt.status})

    async def _stream(
        self, connection: Connection, stream: ActiveStream, log: dict[str, Any]
    ) -> Exception | None:
        streaming = asyncio.ensure_future(
            stream_logs(
                connection,
                stream.endpoint,
                stream.request.level,
                stream.seconds,
                dict(log),
            )
        )
        stopping = asyncio.ensure_future(stream.stopped.wait())
        try:
            await asyncio.wait(
                {streaming, stopping}, return_when=asyncio.FIRST_COMPLETED
            )
        except asyncio.CancelledError:
            streaming.cancel()
            stopping.cancel()
            raise
        stopping.cancel()
        if streaming.done():
            return streaming.result()
        streaming.cancel()
        await asyncio.wait({streaming})
        logger.info("stopped a log stream on request", extra=dict(log))
        return None

    async def drain(self) -> None:
        tasks = [
            stream.task for stream in self._streams.values() if stream.task is not None
        ]
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
