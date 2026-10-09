from __future__ import annotations

import json
import logging
import sys
from collections.abc import Callable
from datetime import UTC, datetime

from opentelemetry import trace
from starlette.types import ASGIApp, Message, Receive, Scope, Send

from .metrics import route_label

tracer = trace.get_tracer("dawn")

STANDARD_ATTRIBUTES = frozenset(
    vars(logging.LogRecord("", 0, "", 0, "", None, None)).keys()
) | {"message", "asctime", "taskName"}


class JsonFormatter(logging.Formatter):
    def format(self, record: logging.LogRecord) -> str:
        moment = datetime.fromtimestamp(record.created, UTC)
        document: dict[str, object] = {
            "time": moment.isoformat(timespec="microseconds").replace("+00:00", "Z"),
            "level": record.levelname.lower(),
            "logger": record.name,
            "message": record.getMessage(),
        }
        for name, value in vars(record).items():
            if name not in STANDARD_ATTRIBUTES and not name.startswith("_"):
                document[name] = value
        if record.exc_info:
            document["exception"] = self.formatException(record.exc_info)
        return json.dumps(document, default=str, ensure_ascii=False)


def configure_logging(level: str) -> None:
    handler = logging.StreamHandler(sys.stdout)
    handler.setFormatter(JsonFormatter())
    root = logging.getLogger()
    root.handlers[:] = [handler]
    root.setLevel(level.upper())
    for name in ("uvicorn", "uvicorn.error", "uvicorn.access", "aiokafka"):
        logging.getLogger(name).handlers[:] = []
        logging.getLogger(name).propagate = True
    logging.getLogger("aiokafka").setLevel("WARNING")


def configure_telemetry(endpoint: str, instance: str) -> Callable[[], None]:
    from opentelemetry import trace
    from opentelemetry._logs import set_logger_provider
    from opentelemetry.exporter.otlp.proto.http._log_exporter import OTLPLogExporter
    from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
    from opentelemetry.sdk._logs import LoggerProvider, LoggingHandler
    from opentelemetry.sdk._logs.export import BatchLogRecordProcessor
    from opentelemetry.sdk.resources import Resource
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.sdk.trace.export import BatchSpanProcessor

    base = endpoint.rstrip("/")
    resource = Resource.create(
        {"service.name": "dawn", "service.instance.id": instance}
    )
    tracer_provider = TracerProvider(resource=resource)
    tracer_provider.add_span_processor(
        BatchSpanProcessor(OTLPSpanExporter(endpoint=f"{base}/v1/traces", timeout=10))
    )
    trace.set_tracer_provider(tracer_provider)
    logger_provider = LoggerProvider(resource=resource)
    logger_provider.add_log_record_processor(
        BatchLogRecordProcessor(OTLPLogExporter(endpoint=f"{base}/v1/logs", timeout=10))
    )
    set_logger_provider(logger_provider)
    logging.getLogger().addHandler(LoggingHandler(logger_provider=logger_provider))
    logging.getLogger(__name__).info(
        "exporting traces and logs over OTLP", extra={"endpoint": base}
    )

    def shutdown() -> None:
        tracer_provider.shutdown()
        logger_provider.shutdown()

    return shutdown


class RequestSpans:
    def __init__(self, application: ASGIApp) -> None:
        self._application = application

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http":
            await self._application(scope, receive, send)
            return
        route = route_label(scope["path"])
        method = scope["method"]
        with tracer.start_as_current_span(
            f"{method} {route}",
            kind=trace.SpanKind.SERVER,
            attributes={"http.request.method": method, "http.route": route},
        ) as span:

            async def observed(message: Message) -> None:
                if message["type"] == "http.response.start":
                    span.set_attribute("http.response.status_code", message["status"])
                await send(message)

            await self._application(scope, receive, observed)
