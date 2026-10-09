from __future__ import annotations

import asyncio
import json
import logging
import random
import ssl
import time
import uuid
from collections.abc import Awaitable, Callable
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Protocol

from aiokafka import AIOKafkaProducer
from aiokafka.errors import KafkaError
from aiokafka.helpers import create_ssl_context

from .config import KafkaSettings, KafkaTopics
from .models import pid_field

logger = logging.getLogger(__name__)

PROCESS_RESULTS_SCHEMA = "dusk.process-results/v1"
PROCESS_OUTPUT_SCHEMA = "dusk.process-output/v1"
FILES_SCHEMA = "dusk.files/v1"
SEND_TIMEOUT_SECONDS = 30.0
MAX_MESSAGE_BYTES = 1_000_000
MAX_ERROR_CHARACTERS = 16384
RETRY_SECONDS = 1.0
RETRY_CAP_SECONDS = 60.0
SSL_FILE_PROPERTIES = ("ssl_cafile", "ssl_certfile", "ssl_keyfile")
SECRET_FILE_PROPERTIES = {"sasl_plain_password_file": "sasl_plain_password"}


def timestamp(nanoseconds: int | None = None) -> str:
    if nanoseconds is None:
        nanoseconds = time.time_ns()
    seconds, fraction = divmod(nanoseconds, 1_000_000_000)
    moment = datetime.fromtimestamp(seconds, UTC)
    return f"{moment:%Y-%m-%dT%H:%M:%S}.{fraction:09d}Z"


def envelope(schema: str) -> dict[str, Any]:
    return {"schema": schema, "id": str(uuid.uuid7()), "time": timestamp()}


def encode(value: dict[str, Any]) -> bytes:
    return json.dumps(
        value,
        separators=(",", ":"),
        ensure_ascii=False,
        allow_nan=False,
        default=str,
    ).encode()


def fitted(message: dict[str, Any]) -> dict[str, Any]:
    size = len(encode(message))
    if size <= MAX_MESSAGE_BYTES:
        return message
    error = message.get("error")
    if isinstance(error, str):
        message["error"] = error[:MAX_ERROR_CHARACTERS]
    reported = message.get("reported")
    if isinstance(reported, dict) and len(encode(message)) > MAX_MESSAGE_BYTES:
        message["reported"] = {**reported, "facts": None}
    if len(encode(message)) > MAX_MESSAGE_BYTES:
        message["reported"] = None
    logger.warning(
        "a process result was larger than one Kafka message; dawn shortened it",
        extra={
            "pid": pid_field(message.get("pid")),
            "size_bytes": size,
            "shortened_bytes": len(encode(message)),
        },
    )
    return message


class Producer(Protocol):
    async def send(
        self, topic: str, key: str, value: dict[str, Any]
    ) -> Awaitable[bool]: ...


class KafkaProducer:
    def __init__(
        self,
        settings: KafkaSettings,
        instance: str,
        failed: Callable[[str], None] = lambda topic: None,
        timeout_seconds: float = SEND_TIMEOUT_SECONDS,
    ) -> None:
        self._timeout_seconds = timeout_seconds
        options: dict[str, Any] = {
            key: value
            for key, value in settings.properties.items()
            if key not in SSL_FILE_PROPERTIES and key not in SECRET_FILE_PROPERTIES
        }
        files = {
            key: str(settings.properties[key])
            for key in SSL_FILE_PROPERTIES
            if key in settings.properties
        }
        self.ssl_files = {key: Path(value) for key, value in files.items()}
        self.ssl_context: ssl.SSLContext | None = None
        if files:
            self.ssl_context = create_ssl_context(
                cafile=files.get("ssl_cafile"),
                certfile=files.get("ssl_certfile"),
                keyfile=files.get("ssl_keyfile"),
            )
            options["ssl_context"] = self.ssl_context
        for file_property, option in SECRET_FILE_PROPERTIES.items():
            path = settings.properties.get(file_property)
            if path is not None:
                with open(str(path)) as file:
                    options[option] = file.read().strip()
        self._producer = AIOKafkaProducer(
            bootstrap_servers=settings.brokers,
            client_id=f"dawn-{instance}",
            acks="all",
            enable_idempotence=True,
            compression_type="zstd",
            linger_ms=5,
            request_timeout_ms=int(SEND_TIMEOUT_SECONDS * 1000),
            max_request_size=MAX_MESSAGE_BYTES + 65536,
            **options,
        )
        self._failed = failed
        self._started = False
        self._failing = False

    async def start(self) -> None:
        await self._producer.start()
        self._started = True
        logger.info("connected to Kafka")

    async def stop(self) -> None:
        if not self._started:
            return
        self._started = False
        try:
            await self._producer.stop()
        except KafkaError as failure:
            logger.error(
                "Kafka did not take every message before dawn stopped",
                extra={"error": str(failure)},
            )
            return
        logger.info("disconnected from Kafka")

    def ready(self) -> bool:
        return self._started and not self._failing

    async def send(
        self, topic: str, key: str, value: dict[str, Any]
    ) -> Awaitable[bool]:
        try:
            async with asyncio.timeout(self._timeout_seconds):
                delivery = await self._producer.send(
                    topic, value=encode(value), key=key.encode()
                )
        except (KafkaError, TimeoutError) as failure:
            self._lost(topic, key, value, failure)
            return _answered(False)

        async def delivered() -> bool:
            try:
                async with asyncio.timeout(self._timeout_seconds):
                    await delivery
            except (KafkaError, TimeoutError) as failure:
                self._lost(topic, key, value, failure)
                return False
            if self._failing:
                self._failing = False
                logger.info("Kafka takes dawn's messages again")
            return True

        return delivered()

    def _lost(
        self, topic: str, key: str, value: dict[str, Any], failure: BaseException
    ) -> None:
        self._failing = True
        self._failed(topic)
        logger.error(
            "Kafka did not take a message",
            extra={
                "topic": topic,
                "key": key,
                "message_id": value.get("id"),
                "pid": pid_field(value.get("pid")),
                "status": value.get("status"),
                "error": str(failure) or type(failure).__name__,
            },
        )


async def _answered(delivered: bool) -> bool:
    return delivered


class Events:
    def __init__(self, producer: Producer, topics: KafkaTopics, instance: str) -> None:
        self._producer = producer
        self._topics = topics
        self._instance = instance

    @property
    def instance(self) -> str:
        return self._instance

    async def _deliver(self, topic: str, key: str, message: dict[str, Any]) -> None:
        attempt = 0
        while not await (await self._producer.send(topic, key, message)):
            delay = random.uniform(
                0, min(RETRY_CAP_SECONDS, RETRY_SECONDS * 2**attempt)
            )
            attempt += 1
            logger.warning(
                "sending a message Kafka did not take again",
                extra={
                    "topic": topic,
                    "message_id": message["id"],
                    "pid": pid_field(message.get("pid")),
                    "attempt": attempt,
                    "retry_in_seconds": round(delay, 3),
                },
            )
            await asyncio.sleep(delay)

    async def process_result(self, **fields: Any) -> None:
        message = envelope(PROCESS_RESULTS_SCHEMA)
        message.update(fields)
        message["dawn_instance"] = self._instance
        key = f"{fields['device_id']}/{fields['installation_id']}"
        await self._deliver(self._topics.process_results, key, fitted(message))
        logger.info(
            "produced a process result",
            extra={
                "pid": pid_field(fields["pid"]),
                "campaign_id": fields.get("campaign_id"),
                "attempt": fields.get("attempt"),
                "device_id": fields["device_id"],
                "installation_id": fields["installation_id"],
                "namespace_id": fields["namespace_id"],
                "action_kind": fields["action_kind"],
                "status": fields["status"],
                "delivered": fields["delivered"],
            },
        )

    async def process_output(self, **fields: Any) -> Awaitable[bool]:
        message = envelope(PROCESS_OUTPUT_SCHEMA)
        message.update(fields)
        key = f"{fields['device_id']}/{fields['installation_id']}"
        return await self._producer.send(self._topics.process_output, key, message)

    async def file(self, **fields: Any) -> None:
        message = envelope(FILES_SCHEMA)
        message.update(fields)
        message["dawn_instance"] = self._instance
        key = f"{fields['device_id']}/{fields['installation_id']}"
        await self._deliver(self._topics.files, key, message)
