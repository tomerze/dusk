from __future__ import annotations

import json
import logging

from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import PlainTextResponse
from starlette.routing import Route
from starlette.testclient import TestClient

from dawn import telemetry
from dawn.telemetry import JsonFormatter, RequestSpans


def test_a_log_line_is_one_json_document_with_its_ids():
    record = logging.LogRecord(
        "dawn.work", logging.INFO, __file__, 1, "finished work on a node", None, None
    )
    record.pid = "9e3779b97f4a7c15"
    record.attempt = 2

    document = json.loads(JsonFormatter().format(record))

    assert document["level"] == "info"
    assert document["logger"] == "dawn.work"
    assert document["message"] == "finished work on a node"
    assert document["pid"] == "9e3779b97f4a7c15"
    assert document["attempt"] == 2
    assert document["time"].endswith("Z")
    assert "args" not in document and "msecs" not in document


def test_an_exception_is_carried_in_the_log_line():
    try:
        raise ValueError("broken")
    except ValueError:
        import sys

        record = logging.LogRecord(
            "dawn", logging.ERROR, __file__, 1, "failed", None, sys.exc_info()
        )

    document = json.loads(JsonFormatter().format(record))

    assert "ValueError: broken" in document["exception"]


def ok(request: Request) -> PlainTextResponse:
    return PlainTextResponse("ok", status_code=202)


def test_every_request_is_a_server_span_named_by_its_route(monkeypatch):
    exporter = InMemorySpanExporter()
    provider = TracerProvider()
    provider.add_span_processor(SimpleSpanProcessor(exporter))
    monkeypatch.setattr(telemetry, "tracer", provider.get_tracer("dawn"))

    with TestClient(
        RequestSpans(Starlette(routes=[Route("/v1/files", ok, methods=["POST"])]))
    ) as client:
        client.post("/v1/files")

    (span,) = exporter.get_finished_spans()
    assert span.name == "POST /v1/files"
    assert span.attributes is not None
    assert span.attributes["http.response.status_code"] == 202
    assert span.attributes["http.route"] == "/v1/files"
