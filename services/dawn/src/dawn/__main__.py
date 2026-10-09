from __future__ import annotations

import argparse
import asyncio
import contextlib
import logging
import random
import signal
import sys
import time
from collections.abc import Awaitable, Callable
from pathlib import Path
from typing import Any

import httpx
from aiokafka.errors import KafkaError
from pydantic import ValidationError

from . import binding
from .api import Services, build_application
from .auth import Authenticator, OidcVerifier, TokenFile
from .config import Settings, load, split_host_port
from .dispatch import Dispatcher
from .events import Events, KafkaProducer
from .file_limit import FileLimitTooLow, raise_file_limit
from .files import Files, s3_client
from .keys import UrlKeys, fetched
from .logstreams import LogStreams
from .metrics import Metrics
from .nodes import Connector, Sessions
from .server import (
    main_server,
    metrics_application,
    plain_server,
    reload_certificates,
    server_ssl_context,
)
from .telemetry import configure_logging, configure_telemetry
from .work import Results, Runner

logger = logging.getLogger("dawn")

MIN_OUTPUT_KEY_BYTES = 32
OIDC_KEYS_CACHE_SECONDS = 300.0
KAFKA_BACKOFF_SECONDS = 1.0
KAFKA_BACKOFF_CAP_SECONDS = 60.0
CLOSE_SECONDS = 30.0


def output_key(path: Path) -> bytes:
    key = path.read_bytes().strip()
    if len(key) < MIN_OUTPUT_KEY_BYTES:
        raise ValueError(
            f"{path} holds {len(key)} bytes; the output key needs at least {MIN_OUTPUT_KEY_BYTES}"
        )
    return key


def oidc_verifier(settings: Settings, client: httpx.AsyncClient) -> OidcVerifier | None:
    issuer, audience = settings.auth.oidc_issuer, settings.auth.oidc_audience
    if issuer is None or audience is None:
        return None
    discovery = issuer.rstrip("/") + "/.well-known/openid-configuration"

    async def located() -> str:
        document = await fetched(client, discovery)
        location = document.get("jwks_uri") if isinstance(document, dict) else None
        if not isinstance(location, str) or not location.startswith("https://"):
            raise ValueError(f"{discovery} names no https jwks_uri")
        return location

    return OidcVerifier(
        issuer,
        audience,
        settings.auth.role_claim,
        settings.auth.role_map,
        UrlKeys(located, client, OIDC_KEYS_CACHE_SECONDS),
    )


async def keep_starting(start: Callable[[], Awaitable[None]], name: str) -> None:
    attempt = 0
    while True:
        try:
            await start()
            return
        except (KafkaError, OSError) as failure:
            delay = random.uniform(
                0, min(KAFKA_BACKOFF_CAP_SECONDS, KAFKA_BACKOFF_SECONDS * 2**attempt)
            )
            attempt += 1
            logger.warning(
                f"could not connect to {name}; trying again",
                extra={
                    "error": str(failure) or type(failure).__name__,
                    "retry_in_seconds": round(delay, 3),
                },
            )
            await asyncio.sleep(delay)


async def serve(
    settings: Settings,
    connector: Connector = binding.connect,
    programs: list[dict[str, Any]] | None = None,
) -> None:
    shutdown_telemetry = (
        configure_telemetry(settings.otlp.endpoint, settings.instance)
        if settings.otlp.endpoint is not None
        else None
    )
    metrics = Metrics()
    key = output_key(settings.output_key_file)
    context = server_ssl_context(settings.tls)
    host, port = split_host_port(settings.listen)
    metrics_host, metrics_port = split_host_port(settings.metrics_listen)
    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for handled in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(handled, stop.set)

    async with contextlib.AsyncExitStack() as stack:
        oidc_client = await stack.enter_async_context(httpx.AsyncClient())
        authenticator = Authenticator(
            TokenFile(settings.auth.tokens_file)
            if settings.auth.tokens_file is not None
            else None,
            oidc_verifier(settings, oidc_client),
            settings.principals if settings.tls.client_ca is not None else {},
        )
        producer = KafkaProducer(
            settings.kafka, settings.instance, metrics.kafka_failed
        )
        stack.push_async_callback(producer.stop)
        kafka = asyncio.create_task(keep_starting(producer.start, "Kafka"))
        storage = await stack.enter_async_context(s3_client(settings.s3))
        sessions = Sessions(
            settings.limits.max_node_sessions, metrics.node_sessions.set
        )
        results = Results(
            Events(producer, settings.kafka.topics, settings.instance),
            key,
            metrics.counted,
        )
        limits = settings.limits
        files = Files(
            storage,
            settings.s3.bucket,
            results,
            limits.max_file_bytes,
            limits.max_concurrent_uploads,
            limits.max_staged_bytes,
            metrics.upload_bytes.inc,
        )
        runner = Runner(settings, connector, results, files)
        services = Services(
            settings=settings,
            authenticator=authenticator,
            connector=connector,
            sessions=sessions,
            results=results,
            dispatcher=Dispatcher(settings, runner, sessions),
            files=files,
            log_streams=LogStreams(
                settings, connector, sessions, results, metrics.log_streams.set
            ),
            metrics=metrics,
            programs=programs if programs is not None else binding.programs(),
            kafka_ready=producer.ready,
        )
        api = main_server(
            build_application(services, host), host, port, context, CLOSE_SECONDS
        )
        observer = plain_server(
            metrics_application(metrics, services.readiness), metrics_host, metrics_port
        )
        reloaders = [
            reload_certificates(
                context,
                settings.tls.certificate,
                settings.tls.key,
                settings.tls.client_ca,
                "server",
            )
        ]
        if producer.ssl_context is not None:
            reloaders.append(
                reload_certificates(
                    producer.ssl_context,
                    producer.ssl_files.get("ssl_certfile"),
                    producer.ssl_files.get("ssl_keyfile"),
                    producer.ssl_files.get("ssl_cafile"),
                    "kafka",
                )
            )
        reloading = asyncio.gather(*reloaders)
        serving = [
            asyncio.create_task(api.serve()),
            asyncio.create_task(observer.serve()),
        ]
        logger.info(
            "dawn is serving",
            extra={
                "listen": settings.listen,
                "metrics_listen": settings.metrics_listen,
                "instance": settings.instance,
            },
        )
        stopping = asyncio.create_task(stop.wait())
        await asyncio.wait([stopping, *serving], return_when=asyncio.FIRST_COMPLETED)

        logger.info("dawn is draining", extra={"drain_seconds": settings.drain_seconds})
        began = time.monotonic()
        services.draining = True
        await services.dispatcher.drain(settings.drain_seconds)
        await services.files.drain(
            max(0.0, settings.drain_seconds - (time.monotonic() - began))
        )
        await services.log_streams.drain()
        api.should_exit = True
        observer.should_exit = True
        await asyncio.gather(*serving, return_exceptions=True)
        for task in (reloading, kafka, stopping):
            task.cancel()
        await asyncio.gather(reloading, kafka, stopping, return_exceptions=True)
    for handled in (signal.SIGTERM, signal.SIGINT):
        loop.remove_signal_handler(handled)
    if shutdown_telemetry is not None:
        shutdown_telemetry()
    logger.info("dawn stopped")


def main() -> None:
    parser = argparse.ArgumentParser(
        prog="dawn",
        description="Serve dawn, the client layer of the Dusk stack: it runs work on dusk nodes through nightfall.",
    )
    parser.add_argument(
        "--config",
        type=Path,
        help="the TOML configuration file (default: $DAWN_CONFIG, else /etc/dawn/dawn.toml when it exists)",
    )
    arguments = parser.parse_args()
    try:
        settings = load(arguments.config)
    except (ValueError, ValidationError) as failure:
        print(f"dawn: invalid configuration: {failure}", file=sys.stderr)
        raise SystemExit(2) from failure
    configure_logging(settings.log_level)
    try:
        raise_file_limit(settings.limits.max_node_sessions)
        asyncio.run(serve(settings))
    except (FileLimitTooLow, OSError, ValueError) as failure:
        logger.error("dawn could not start", extra={"error": str(failure)})
        raise SystemExit(1) from failure


if __name__ == "__main__":
    main()
