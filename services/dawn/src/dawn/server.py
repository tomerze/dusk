from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import ssl
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import uvicorn
from prometheus_client import CONTENT_TYPE_LATEST, generate_latest
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse, Response
from starlette.routing import Route
from starlette.types import ASGIApp, Message, Receive, Scope, Send
from uvicorn.protocols.http.h11_impl import H11Protocol

from .auth import PEER_CERTIFICATE
from .config import TlsSettings
from .metrics import Metrics

logger = logging.getLogger(__name__)

MAX_BODY_BYTES = 16 * 1024 * 1024
RELOAD_SECONDS = 30.0


class PeerCertificateProtocol(H11Protocol):
    def connection_made(self, transport: asyncio.Transport) -> None:
        super().connection_made(transport)
        ssl_object = transport.get_extra_info("ssl_object")
        certificate = (
            ssl_object.getpeercert(binary_form=True) if ssl_object is not None else None
        )
        if certificate:
            self.app_state = {**self.app_state, PEER_CERTIFICATE: certificate}


def server_ssl_context(settings: TlsSettings) -> ssl.SSLContext:
    if settings.certificate is None or settings.key is None:
        raise ValueError("tls.certificate and tls.key are required")
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.set_alpn_protocols(["http/1.1"])
    context.load_cert_chain(str(settings.certificate), str(settings.key))
    if settings.client_ca is not None:
        context.verify_mode = ssl.CERT_OPTIONAL
        context.load_verify_locations(str(settings.client_ca))
    return context


def modification_times(paths: list[Path]) -> tuple[int, ...]:
    times = []
    for path in paths:
        try:
            times.append(path.stat().st_mtime_ns)
        except FileNotFoundError:
            times.append(-1)
    return tuple(times)


async def reload_certificates(
    context: ssl.SSLContext,
    certificate: Path | None,
    key: Path | None,
    authorities: Path | None,
    purpose: str,
) -> None:
    paths = [path for path in (certificate, key, authorities) if path is not None]
    seen = modification_times(paths)
    while True:
        await asyncio.sleep(RELOAD_SECONDS)
        try:
            current = modification_times(paths)
            if current == seen:
                continue
            if certificate is not None and key is not None:
                context.load_cert_chain(str(certificate), str(key))
            if authorities is not None:
                context.load_verify_locations(str(authorities))
        except (OSError, ssl.SSLError) as failure:
            logger.warning(
                "kept the certificate loaded before: the new one cannot be read",
                extra={"purpose": purpose, "error": str(failure)},
            )
            continue
        seen = current
        logger.info("loaded a new certificate", extra={"purpose": purpose})


class BodyLimit:
    def __init__(self, application: ASGIApp, limit: int = MAX_BODY_BYTES) -> None:
        self._application = application
        self._limit = limit

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http":
            await self._application(scope, receive, send)
            return
        for name, value in scope.get("headers", []):
            if (
                name == b"content-length"
                and value.isdigit()
                and int(value) > self._limit
            ):
                await _too_large(send, self._limit)
                return
        received = 0

        async def bounded() -> Message:
            nonlocal received
            message = await receive()
            if message["type"] == "http.request":
                received += len(message.get("body", b""))
                if received > self._limit:
                    raise BodyTooLarge
            return message

        try:
            await self._application(scope, bounded, send)
        except BodyTooLarge:
            await _too_large(send, self._limit)


class BodyTooLarge(Exception):
    pass


async def _too_large(send: Send, limit: int) -> None:
    body = json.dumps(
        {"error": f"the request body is larger than {limit} bytes"}
    ).encode()
    await send(
        {
            "type": "http.response.start",
            "status": 413,
            "headers": [
                (b"content-type", b"application/json"),
                (b"content-length", str(len(body)).encode()),
            ],
        }
    )
    await send({"type": "http.response.body", "body": body})


def metrics_application(
    metrics: Metrics, readiness: Callable[[], dict[str, bool]]
) -> Starlette:
    async def exposition(request: Request) -> Response:
        return Response(
            generate_latest(metrics.registry), media_type=CONTENT_TYPE_LATEST
        )

    async def healthz(request: Request) -> Response:
        return JSONResponse({"status": "alive"})

    async def readyz(request: Request) -> Response:
        checks = readiness()
        ready = all(checks.values())
        return JSONResponse(
            {"ready": ready, "checks": checks}, status_code=200 if ready else 503
        )

    return Starlette(
        routes=[
            Route("/metrics", exposition),
            Route("/healthz", healthz),
            Route("/readyz", readyz),
        ]
    )


class Server(uvicorn.Server):
    @contextlib.contextmanager
    def capture_signals(self) -> Iterator[None]:
        yield


def main_server(
    application: ASGIApp,
    host: str,
    port: int,
    context: ssl.SSLContext,
    graceful_seconds: float,
) -> Server:
    def factory(config: Any, default: Any) -> ssl.SSLContext:
        return context

    return Server(
        uvicorn.Config(
            BodyLimit(application),
            host=host,
            port=port,
            http=PeerCertificateProtocol,
            ssl_context_factory=factory,
            proxy_headers=False,
            server_header=False,
            access_log=False,
            log_config=None,
            lifespan="on",
            timeout_graceful_shutdown=int(graceful_seconds),
        )
    )


def plain_server(application: ASGIApp, host: str, port: int) -> Server:
    return Server(
        uvicorn.Config(
            application,
            host=host,
            port=port,
            http="h11",
            proxy_headers=False,
            server_header=False,
            access_log=False,
            log_config=None,
            lifespan="off",
        )
    )
