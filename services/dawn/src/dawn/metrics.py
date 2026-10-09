from __future__ import annotations

import time

from prometheus_client import CollectorRegistry, Counter, Gauge, Histogram
from starlette.types import ASGIApp, Message, Receive, Scope, Send

ROUTES = (
    "/v1/dispatch",
    "/v1/reap",
    "/v1/facts",
    "/v1/files",
    "/v1/connect",
    "/v1/disconnect",
    "/v1/sh/stream",
    "/v1/sh",
    "/v1/help",
    "/v1/openapi.json",
    "/v1/docs",
    "/v1/static",
    "/v1/logs",
    "/mcp",
    "/healthz",
    "/readyz",
)


METHODS = frozenset({"GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"})


def method_label(method: str) -> str:
    return method if method in METHODS else "other"


def route_label(path: str) -> str:
    for route in ROUTES:
        if path == route or path.startswith(route + "/"):
            return route
    return "other"


class Metrics:
    def __init__(self, registry: CollectorRegistry | None = None) -> None:
        self.registry = registry or CollectorRegistry()
        self.requests = Counter(
            "dawn_requests",
            "HTTP requests dawn answered",
            ["route", "method", "status"],
            registry=self.registry,
        )
        self.request_seconds = Histogram(
            "dawn_request_duration_seconds",
            "Time dawn took to answer an HTTP request, to the end of its body",
            ["route"],
            registry=self.registry,
        )
        self.process_results = Counter(
            "dawn_process_results",
            "Final process results dawn produced",
            ["action_kind", "status"],
            registry=self.registry,
        )
        self.node_sessions = Gauge(
            "dawn_node_sessions", "Node sessions dawn holds", registry=self.registry
        )
        self.log_streams = Gauge(
            "dawn_log_streams", "Log streams dawn runs", registry=self.registry
        )
        self.upload_bytes = Counter(
            "dawn_upload_bytes",
            "Bytes of collected files dawn sent to S3",
            registry=self.registry,
        )
        self.kafka_failures = Counter(
            "dawn_kafka_produce_failures",
            "Messages Kafka did not take",
            ["topic"],
            registry=self.registry,
        )

    def counted(self, action_kind: str, status: str) -> None:
        self.process_results.labels(action_kind, status).inc()

    def kafka_failed(self, topic: str) -> None:
        self.kafka_failures.labels(topic).inc()


class RequestMetrics:
    def __init__(self, application: ASGIApp, metrics: Metrics) -> None:
        self._application = application
        self._metrics = metrics

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http":
            await self._application(scope, receive, send)
            return
        began = time.monotonic()
        status = 500

        async def observed(message: Message) -> None:
            nonlocal status
            if message["type"] == "http.response.start":
                status = message["status"]
            await send(message)

        route = route_label(scope["path"])
        try:
            await self._application(scope, receive, observed)
        finally:
            self._metrics.requests.labels(
                route, method_label(scope["method"]), str(status)
            ).inc()
            self._metrics.request_seconds.labels(route).observe(
                time.monotonic() - began
            )
