from __future__ import annotations

import asyncio
import functools
import logging
from dataclasses import dataclass, field

from .config import Settings
from .models import DispatchRequest, NodeRef, ReapRequest, Work, pid_field
from .nodes import Sessions
from .work import Runner, ordered

logger = logging.getLogger(__name__)


class QueueFull(Exception):
    pass


class Draining(Exception):
    pass


@dataclass(frozen=True)
class Reap:
    node: NodeRef
    pids: list[int]


@dataclass
class NodeQueue:
    pending: list[tuple[NodeRef, Work]] = field(default_factory=list)
    reaps: list[Reap] = field(default_factory=list)
    pids: set[str] = field(default_factory=set)
    started: set[str] = field(default_factory=set)
    running: list[tuple[NodeRef, Work]] = field(default_factory=list)
    script_bytes: int = 0
    task: asyncio.Task[None] | None = None


def script_bytes(work: Work) -> int:
    return len(work.script.encode()) if work.script is not None else 0


class Dispatcher:
    def __init__(
        self,
        settings: Settings,
        runner: Runner,
        sessions: Sessions,
    ) -> None:
        self._settings = settings
        self._runner = runner
        self._sessions = sessions
        self._limit = settings.limits.max_work_per_node
        self._script_limit = settings.limits.max_queued_script_bytes
        self._script_bytes = 0
        self._nodes: dict[tuple[str, str], NodeQueue] = {}
        self._draining = False

    def active(self) -> int:
        return len(self._nodes)

    def submit(self, request: DispatchRequest) -> list[str]:
        self._check_accepting()
        if len(request.work) > self._limit:
            raise ValueError(
                f"a dispatch holds at most {self._limit} processes "
                "(limits.max_work_per_node)"
            )
        node = request.node
        queue = self._nodes.get((node.device_id, node.installation_id))
        held = queue.pids if queue is not None else set()
        fresh = [work for work in request.work if work.pid not in held]
        waiting = len(queue.pending) if queue is not None else 0
        if waiting + len(fresh) > self._limit:
            raise QueueFull(
                f"node {node.device_id}/{node.installation_id} already has {waiting} "
                f"processes waiting, and holds at most {self._limit}"
            )
        weight = sum(script_bytes(work) for work in fresh)
        if self._script_bytes + weight > self._script_limit:
            raise QueueFull(
                f"dawn holds {self._script_bytes} bytes of scripts waiting to run, "
                f"and at most {self._script_limit} (limits.max_queued_script_bytes)"
            )
        queue = self._queue(node)
        queue.pending.extend((node, work) for work in fresh)
        queue.pids.update(work.pid for work in fresh)
        queue.script_bytes += weight
        self._script_bytes += weight
        logger.info(
            "accepted a dispatch",
            extra={
                "device_id": node.device_id,
                "installation_id": node.installation_id,
                "namespace_id": node.namespace_id,
                "pids": [pid_field(work.pid) for work in request.work],
                "already_held": len(request.work) - len(fresh),
                "waiting": len(queue.pending),
            },
        )
        return [work.pid for work in request.work]

    def submit_reap(self, request: ReapRequest) -> list[str]:
        self._check_accepting()
        node = request.node
        queue = self._nodes.get((node.device_id, node.installation_id))
        if queue is not None and len(queue.reaps) >= self._limit:
            raise QueueFull(
                f"node {node.device_id}/{node.installation_id} already has "
                f"{len(queue.reaps)} reaps waiting, and holds at most {self._limit}"
            )
        pids = list(dict.fromkeys(request.pids))
        self._queue(node).reaps.append(Reap(node, [int(pid) for pid in pids]))
        logger.info(
            "accepted a reap",
            extra={
                "device_id": node.device_id,
                "installation_id": node.installation_id,
                "namespace_id": node.namespace_id,
                "pids": [pid_field(pid) for pid in pids],
            },
        )
        return pids

    def _check_accepting(self) -> None:
        if self._draining:
            raise Draining("dawn is shutting down and takes no new dispatches")

    def _queue(self, node: NodeRef) -> NodeQueue:
        key = (node.device_id, node.installation_id)
        queue = self._nodes.get(key)
        if queue is None:
            self._sessions.reserve()
            queue = NodeQueue()
            self._nodes[key] = queue
            queue.task = asyncio.get_running_loop().create_task(self._work(key, queue))
        return queue

    async def _work(self, key: tuple[str, str], queue: NodeQueue) -> None:
        try:
            while queue.pending or queue.reaps:
                reaps, queue.reaps = queue.reaps, []
                for reap in reaps:
                    await self._runner.reap(reap.node, reap.pids)
                queue.running, queue.pending = queue.pending, []
                queue.started.clear()
                for node, work in ordered(queue.running):
                    queue.started.add(work.pid)
                    try:
                        await self._runner.run(
                            node,
                            work,
                            functools.partial(queue.pids.discard, work.pid),
                        )
                    finally:
                        queue.script_bytes -= script_bytes(work)
                        self._script_bytes -= script_bytes(work)
                queue.running = []
        except asyncio.CancelledError:
            unstarted = [
                (node, work)
                for node, work in queue.running + queue.pending
                if work.pid not in queue.started
            ]
            stopped = RuntimeError("dawn stopped before running the process")
            for node, work in unstarted:
                await self._runner.unreachable(
                    node, work, stopped, despite_cancellation=True
                )
            for reap in queue.reaps:
                logger.warning(
                    "dawn stopped before reaping processes on a node",
                    extra={
                        "device_id": key[0],
                        "installation_id": key[1],
                        "pids": [pid_field(pid) for pid in reap.pids],
                    },
                )
            raise
        except Exception:
            logger.exception(
                "a node's dispatch worker failed",
                extra={"device_id": key[0], "installation_id": key[1]},
            )
        finally:
            self._script_bytes -= queue.script_bytes
            del self._nodes[key]
            self._sessions.release()

    async def drain(self, seconds: float) -> None:
        self._draining = True
        tasks = [queue.task for queue in self._nodes.values() if queue.task is not None]
        if not tasks:
            return
        logger.info(
            "waiting for dispatches to finish",
            extra={"nodes": len(tasks), "seconds": seconds},
        )
        _, unfinished = await asyncio.wait(tasks, timeout=seconds)
        if not unfinished:
            logger.info("every dispatch finished")
            return
        logger.warning(
            "stopping dispatches that did not finish in time",
            extra={"nodes": len(unfinished)},
        )
        for task in unfinished:
            task.cancel()
        await asyncio.gather(*unfinished, return_exceptions=True)
