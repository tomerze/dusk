from __future__ import annotations

import asyncio
import contextlib
import hashlib
import logging
import os
import tempfile
import time
import uuid
from collections.abc import Callable
from typing import Any

from aiobotocore.config import AioConfig
from aiobotocore.session import get_session

from . import facts
from .config import S3Settings, Settings
from .events import timestamp
from .models import FileRequest, NodeRef, file_name
from .nodes import Connection, Connector, Output, Sessions, described
from .work import Attempt, Results, classify, shell_at

logger = logging.getLogger(__name__)

PART_BYTES = 8 * 1024 * 1024
CONTENT_TYPE = "application/octet-stream"
ABORT_SECONDS = 30.0
WATCH_SECONDS = 0.25


class UploadsExhausted(Exception):
    pass


def object_key(node: NodeRef, pid: int, index: int, path: str) -> str:
    return (
        f"files/{node.device_id}/{node.installation_id}/{pid}/{index}-{file_name(path)}"
    )


async def copied_record(output: Output) -> dict[str, Any]:
    copied: dict[str, Any] = {}
    async for value in facts.values(output):
        fields = facts.record_fields(value)
        if fields is not None and "sha256" in fields:
            copied = fields
    return copied


def s3_client(settings: S3Settings) -> Any:
    credentials: dict[str, str] = {}
    if settings.access_key_file is not None and settings.secret_key_file is not None:
        credentials = {
            "aws_access_key_id": settings.access_key_file.read_text().strip(),
            "aws_secret_access_key": settings.secret_key_file.read_text().strip(),
        }
    return get_session().create_client(
        "s3",
        endpoint_url=settings.endpoint,
        region_name=settings.region,
        verify=str(settings.ca) if settings.ca is not None else True,
        config=AioConfig(
            s3={"addressing_style": "path"},
            connect_timeout=10,
            read_timeout=60,
            retries={"max_attempts": 5, "mode": "standard"},
            request_checksum_calculation="when_required",
            response_checksum_validation="when_required",
        ),
        **credentials,
    )


def quoted(word: str) -> str:
    return f"'{word}'" if "'" not in word else f'"{word}"'


def verified(path: str, expected: str | None, actual: str) -> None:
    if expected is not None and expected.lower() != actual:
        raise ValueError(
            f"the node copied {path} with SHA-256 {expected}, dawn read {actual}"
        )


def removed(path: str) -> None:
    with contextlib.suppress(FileNotFoundError):
        os.unlink(path)


class Files:
    def __init__(
        self,
        client: Any,
        bucket: str,
        results: Results,
        max_file_bytes: int,
        max_concurrent: int,
        max_staged_bytes: int,
        uploaded: Callable[[int], None] = lambda size: None,
        part_bytes: int = PART_BYTES,
        clock: Callable[[], float] = time.monotonic,
        directory: str | None = None,
    ) -> None:
        self._client = client
        self._bucket = bucket
        self._results = results
        self._max_file_bytes = max_file_bytes
        self._max_concurrent = min(max_concurrent, max_staged_bytes // max_file_bytes)
        self._slots = asyncio.Semaphore(self._max_concurrent)
        self._uploaded = uploaded
        self._part_bytes = part_bytes
        self._clock = clock
        self._directory = directory
        self._tasks: set[asyncio.Task[None]] = set()
        self._uploads: dict[tuple[str, str, str, str], str] = {}

    async def collect(
        self,
        connection: Connection,
        attempt: Attempt,
        path: str,
        index: int,
        deadline: float,
    ) -> None:
        attempt.delivered = True
        try:
            async with asyncio.timeout(deadline - self._clock()):
                async with self._slots:
                    await self._collect(connection, attempt, path, index)
        except asyncio.CancelledError:
            attempt.status = "error"
            attempt.error = "dawn stopped before the file was collected"
            attempt.known = True
            await self._results.finished_despite_cancellation(attempt)
            raise
        except Exception as failure:
            self._failed(attempt, path, failure)
        attempt.known = True
        await self._results.finished(attempt)

    def _failed(self, attempt: Attempt, path: str, failure: BaseException) -> None:
        attempt.status = classify(failure, attempt.delivered)
        attempt.error = described(failure)
        logger.warning(
            "could not collect a file",
            extra={
                **attempt.log(),
                "node_path": path,
                "status": attempt.status,
                "error": attempt.error,
            },
        )

    async def _collect(
        self, connection: Connection, attempt: Attempt, path: str, index: int
    ) -> None:
        node = attempt.node
        key = object_key(node, attempt.pid, index, path)
        descriptor, temporary = tempfile.mkstemp(
            prefix="dawn-", suffix=".part", dir=self._directory
        )
        os.close(descriptor)
        try:
            copied = await self._copy(connection, path, temporary)
            expected = copied.get("sha256")
            size, sha256, parts = await self._upload(
                key,
                temporary,
                attempt,
                path,
                expected if isinstance(expected, str) else None,
            )
        finally:
            removed(temporary)
        await self._results.events.file(
            pid=str(attempt.pid),
            campaign_id=attempt.campaign_id,
            device_id=node.device_id,
            installation_id=node.installation_id,
            namespace_id=node.namespace_id,
            node_path=path,
            bucket=self._bucket,
            object_key=key,
            size_bytes=size,
            sha256=sha256,
            content_type=CONTENT_TYPE,
            uploaded_at=timestamp(),
        )
        attempt.status = "succeeded"
        logger.info(
            "collected a file",
            extra={
                **attempt.log(),
                "node_path": path,
                "object_key": key,
                "size_bytes": size,
                "parts": parts,
            },
        )

    async def _copy(
        self, connection: Connection, path: str, temporary: str
    ) -> dict[str, Any]:
        output = connection.sh(f"cp {quoted(':' + path)} {quoted(temporary)}")
        reading = asyncio.ensure_future(copied_record(output))
        try:
            while True:
                done, _ = await asyncio.wait({reading}, timeout=WATCH_SECONDS)
                if done:
                    copied = reading.result()
                    break
                if os.stat(temporary).st_size > self._max_file_bytes:
                    raise ValueError(
                        f"{path} is larger than limits.max_file_bytes "
                        f"({self._max_file_bytes})"
                    )
        except BaseException:
            if not reading.done():
                reading.cancel()
                logger.warning(
                    "stopping a copy from a node by closing its connection",
                    extra={"node_path": path, **connection.fields},
                )
                await connection.close()
            raise
        size = os.stat(temporary).st_size
        if size > self._max_file_bytes:
            raise ValueError(
                f"{path} is larger than limits.max_file_bytes ({self._max_file_bytes})"
            )
        if copied.get("length") not in (None, size):
            raise ValueError(
                f"the node copied {copied['length']} bytes of {path}, dawn received {size}"
            )
        return copied

    async def _upload(
        self,
        key: str,
        temporary: str,
        attempt: Attempt,
        path: str,
        expected: str | None,
    ) -> tuple[int, str, int]:
        digest = hashlib.sha256()
        size = 0
        upload_id: str | None = None
        parts: list[dict[str, Any]] = []
        with open(temporary, "rb") as file:
            if os.fstat(file.fileno()).st_size <= self._part_bytes:
                body = await asyncio.to_thread(file.read)
                digest.update(body)
                verified(path, expected, digest.hexdigest())
                await self._client.put_object(
                    Bucket=self._bucket, Key=key, Body=body, ContentType=CONTENT_TYPE
                )
                self._uploaded(len(body))
                return len(body), digest.hexdigest(), 1
            try:
                created = await self._client.create_multipart_upload(
                    Bucket=self._bucket, Key=key, ContentType=CONTENT_TYPE
                )
                upload_id = str(created["UploadId"])
                while part := await asyncio.to_thread(file.read, self._part_bytes):
                    digest.update(part)
                    size += len(part)
                    parts.append(await self._part(key, upload_id, len(parts) + 1, part))
                verified(path, expected, digest.hexdigest())
                await self._client.complete_multipart_upload(
                    Bucket=self._bucket,
                    Key=key,
                    UploadId=upload_id,
                    MultipartUpload={"Parts": parts},
                )
            except BaseException:
                if upload_id is not None:
                    await self._abort(key, upload_id, attempt)
                raise
        return size, digest.hexdigest(), len(parts)

    async def _part(
        self, key: str, upload_id: str, number: int, body: bytes
    ) -> dict[str, Any]:
        response = await self._client.upload_part(
            Bucket=self._bucket,
            Key=key,
            UploadId=upload_id,
            PartNumber=number,
            Body=body,
        )
        self._uploaded(len(body))
        return {"ETag": response["ETag"], "PartNumber": number}

    async def _abort(self, key: str, upload_id: str, attempt: Attempt) -> None:
        try:
            async with asyncio.timeout(ABORT_SECONDS):
                await asyncio.shield(
                    self._client.abort_multipart_upload(
                        Bucket=self._bucket, Key=key, UploadId=upload_id
                    )
                )
        except (Exception, asyncio.CancelledError) as failure:
            logger.error(
                "could not abort a multipart upload; the bucket's lifecycle rule must clean it up",
                extra={
                    **attempt.log(),
                    "object_key": key,
                    "upload_id": upload_id,
                    "error": described(failure),
                },
            )
            return
        logger.info(
            "aborted a multipart upload",
            extra={**attempt.log(), "object_key": key, "upload_id": upload_id},
        )

    def start(
        self,
        request: FileRequest,
        settings: Settings,
        connector: Connector,
        sessions: Sessions,
    ) -> str:
        node = request.node
        key = (node.device_id, node.installation_id, request.pid, request.path)
        running = self._uploads.get(key)
        if running is not None:
            return running
        if len(self._tasks) >= self._max_concurrent:
            raise UploadsExhausted(
                f"dawn is already collecting {self._max_concurrent} files, its limit of "
                "limits.max_concurrent_uploads and limits.max_staged_bytes"
            )
        sessions.reserve()
        upload_id = str(uuid.uuid4())
        self._uploads[key] = upload_id
        task = asyncio.get_running_loop().create_task(
            self._standalone(request, settings, connector, sessions, upload_id, key)
        )
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)
        return upload_id

    async def _standalone(
        self,
        request: FileRequest,
        settings: Settings,
        connector: Connector,
        sessions: Sessions,
        upload_id: str,
        key: tuple[str, str, str, str],
    ) -> None:
        attempt = self._results.attempt(
            request.node, int(request.pid), request.campaign_id, None, "collect_file"
        )
        deadline = self._clock() + settings.limits.process_timeout_default
        log = {**attempt.log(), "upload_id": upload_id}
        try:
            async with shell_at(
                connector, settings, request.node, attempt.pid, log
            ) as connection:
                await self.collect(connection, attempt, request.path, 0, deadline)
        except asyncio.CancelledError:
            if not attempt.known:
                attempt.error = "dawn stopped before the file was collected"
                await self._results.finished_despite_cancellation(attempt)
            raise
        except Exception as failure:
            if not attempt.known:
                self._failed(attempt, request.path, failure)
                await self._results.finished(attempt)
        finally:
            self._uploads.pop(key, None)
            sessions.release()

    async def drain(self, seconds: float) -> None:
        if not self._tasks:
            return
        _, unfinished = await asyncio.wait(set(self._tasks), timeout=seconds)
        for task in unfinished:
            task.cancel()
        with contextlib.suppress(asyncio.CancelledError):
            await asyncio.gather(*unfinished, return_exceptions=True)
