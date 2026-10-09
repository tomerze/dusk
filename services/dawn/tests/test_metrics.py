from __future__ import annotations

import pytest
from prometheus_client import generate_latest
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import PlainTextResponse
from starlette.routing import Route
from starlette.testclient import TestClient

from dawn.metrics import Metrics, RequestMetrics, method_label, route_label


@pytest.mark.parametrize(
    ("path", "label"),
    [
        ("/v1/dispatch", "/v1/dispatch"),
        ("/v1/logs/7d1c0f4e-0000-4000-8000-000000000000", "/v1/logs"),
        ("/v1/sh/stream", "/v1/sh/stream"),
        ("/v1/sh", "/v1/sh"),
        ("/v1/static/swagger-ui.css", "/v1/static"),
        ("/v1/help/ps", "/v1/help"),
        ("/v1/shout", "other"),
        ("/random/path/that/must/not/become/a/label", "other"),
    ],
)
def test_a_path_is_labelled_by_its_route_so_labels_stay_bounded(path, label):
    assert route_label(path) == label


@pytest.mark.parametrize(
    ("method", "label"),
    [("GET", "GET"), ("DELETE", "DELETE"), ("BREW", "other"), ("get", "other")],
)
def test_a_method_dawn_does_not_name_is_labelled_other(method, label):
    assert method_label(method) == label


def teapot(request: Request) -> PlainTextResponse:
    return PlainTextResponse("short and stout", status_code=418)


def test_requests_are_counted_by_route_method_and_status():
    metrics = Metrics()
    application = RequestMetrics(Starlette(routes=[Route("/v1/help", teapot)]), metrics)

    with TestClient(application) as client:
        client.get("/v1/help")
        client.get("/v1/help")
        client.get("/nowhere")

    exposed = generate_latest(metrics.registry).decode()
    assert (
        'dawn_requests_total{method="GET",route="/v1/help",status="418"} 2.0' in exposed
    )
    assert 'dawn_requests_total{method="GET",route="other",status="404"} 1.0' in exposed
    assert 'dawn_request_duration_seconds_count{route="/v1/help"} 2.0' in exposed


def test_results_sessions_streams_uploads_and_kafka_failures_are_exposed():
    metrics = Metrics()

    metrics.counted("run_script", "succeeded")
    metrics.node_sessions.set(3)
    metrics.log_streams.set(1)
    metrics.upload_bytes.inc(1024)
    metrics.kafka_failed("dusk.process-results")

    exposed = generate_latest(metrics.registry).decode()
    assert (
        'dawn_process_results_total{action_kind="run_script",status="succeeded"} 1.0'
        in exposed
    )
    assert "dawn_node_sessions 3.0" in exposed
    assert "dawn_log_streams 1.0" in exposed
    assert "dawn_upload_bytes_total 1024.0" in exposed
    assert (
        'dawn_kafka_produce_failures_total{topic="dusk.process-results"} 1.0' in exposed
    )
