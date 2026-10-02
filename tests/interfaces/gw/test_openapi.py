"""The API describes itself: the OpenAPI document and the Swagger UI over it.

The document is generated from the same models that validate requests, so these
tests are not checking that it was written down correctly - they check the things
generation does not guarantee: that it is reachable under the mount, that it
describes every route and no others, and that it does not promise behaviour the
API cannot deliver.
"""

from __future__ import annotations

import re

import pytest
from starlette.testclient import TestClient

OPERATIONS = {
    ("/connect", "post"),
    ("/disconnect", "post"),
    ("/sh", "post"),
    ("/sh/stream", "post"),
    ("/help", "get"),
    ("/help/{program}", "get"),
}
"""Every operation the REST API has, as the spec addresses them under its mount."""


@pytest.fixture
def spec(client: TestClient) -> dict:
    response = client.get("/v1/openapi.json")
    assert response.status_code == 200, response.text
    return response.json()


def test_the_document_is_served_under_the_mount(spec: dict):
    assert spec["openapi"].startswith("3.")
    assert spec["info"]["title"] == "Dusk API gateway"
    assert spec["info"]["version"] == "1"
    # Paths are relative to the mount, which the server entry supplies, so a
    # client generated from this document targets /v1/... and not /...
    assert spec["servers"] == [{"url": "/v1"}]


def test_the_document_describes_every_endpoint_and_no_others(spec: dict):
    described = {
        (path, method)
        for path, operations in spec["paths"].items()
        for method in operations
    }

    assert described == OPERATIONS


def test_request_bodies_are_documented_as_the_strict_types_they_are(spec: dict):
    connect = spec["components"]["schemas"]["ConnectRequest"]

    assert connect["properties"]["host"]["type"] == "string"
    assert connect["properties"]["port"]["type"] == "integer"
    assert sorted(connect["required"]) == ["host", "port"]
    # extra="forbid" on the request models, so the document says so too and a
    # generated client will not send fields the API rejects.
    assert connect["additionalProperties"] is False


@pytest.mark.parametrize(
    ("path", "method", "statuses"),
    [
        ("/connect", "post", {"200", "400", "502"}),
        ("/disconnect", "post", {"200", "400", "404"}),
        ("/sh", "post", {"200", "400", "404", "502"}),
        ("/sh/stream", "post", {"200", "400", "404"}),
        ("/help", "get", {"200"}),
        ("/help/{program}", "get", {"200", "404"}),
    ],
)
def test_each_endpoint_documents_exactly_what_it_can_return(
    spec: dict, path: str, method: str, statuses: set[str]
):
    # Exactly, in both directions. A 404 on /connect would promise a failure
    # that endpoint cannot produce - it mints descriptors rather than reading
    # them - and a missing 502 would leave a caller unprepared for one it will
    # meet the first time a node is busy.
    assert set(spec["paths"][path][method]["responses"]) == statuses


def test_the_streaming_endpoint_advertises_only_the_stream(spec: dict):
    # FastAPI adds application/json from the route's default response class.
    # Documenting it here would promise a JSON body this endpoint never sends.
    assert list(spec["paths"]["/sh/stream"]["post"]["responses"]["200"]["content"]) == [
        "text/event-stream"
    ]


def test_every_failure_is_reported_in_the_one_error_shape(spec: dict):
    error = spec["components"]["schemas"]["ErrorResponse"]
    assert error["properties"]["error"]["type"] == "string"

    for path, operations in spec["paths"].items():
        for method, operation in operations.items():
            for status, response in operation["responses"].items():
                if status.startswith("2"):
                    continue
                schema = response["content"]["application/json"]["schema"]
                assert schema == {"$ref": "#/components/schemas/ErrorResponse"}, (
                    path,
                    method,
                    status,
                )


def test_no_endpoint_promises_a_422_the_api_never_sends(spec: dict):
    # FastAPI documents a 422 on anything it validates, but a rejected body is
    # reported as 400 here. A documented status no caller can receive is a lie
    # in the contract, so it is stripped - along with the schemas describing it.
    for path, operations in spec["paths"].items():
        for method, operation in operations.items():
            assert "422" not in operation["responses"], (path, method)

    schemas = spec["components"]["schemas"]
    assert "HTTPValidationError" not in schemas
    assert "ValidationError" not in schemas


def test_a_rejected_body_is_reported_the_way_the_document_says(client: TestClient):
    response = client.post("/v1/connect", json={"host": "10.0.0.1", "port": "9090"})

    assert response.status_code == 400
    assert set(response.json()) == {"error"}
    assert "port" in response.json()["error"]


def test_an_unexpected_field_is_rejected_rather_than_ignored(client: TestClient):
    response = client.post(
        "/v1/connect", json={"host": "10.0.0.1", "port": 9090, "user": "root"}
    )

    assert response.status_code == 400
    assert "user" in response.json()["error"]


def test_mcp_is_not_described_by_the_rest_document(spec: dict):
    assert not any(path.startswith("/mcp") for path in spec["paths"])


def test_the_browsable_interface_is_served(client: TestClient):
    response = client.get("/v1/docs")

    assert response.status_code == 200
    assert response.headers["content-type"].startswith("text/html")
    # It has to point at the document under the mount, not at the root, or the
    # page renders empty.
    assert "/v1/openapi.json" in response.text


def test_the_browsable_interface_reaches_nothing_outside_this_gateway(
    client: TestClient,
):
    """The property that makes `/v1/docs` usable on an air-gapped host.

    Not "the assets are vendored" - that is the mechanism. This is the outcome:
    nothing on the page is fetched from anywhere but this process, so it renders
    the same with no route to the internet. FastAPI's built-in page would fail
    this on three counts: its script, its stylesheet, and its favicon.
    """
    page = client.get("/v1/docs").text

    external = re.findall(r"""["'(](https?://[^"')\s]+)""", page)

    assert external == [], external


def test_the_swagger_validator_badge_is_turned_off(client: TestClient):
    """Swagger UI phones a third party for the badge unless told not to.

    Its default ``validatorUrl`` is ``validator.swagger.io``, which it is handed
    the address of this API's document - a request that cannot succeed on an
    isolated network, and one that hands an internal API's URL to a stranger
    anywhere else. The setting lives inside the page's script, not in a src or
    href, so the external-URL check above does not cover it.
    """
    page = client.get("/v1/docs").text

    assert '"validatorUrl": null' in page
    assert "validator.swagger.io" not in page


@pytest.mark.parametrize(
    ("asset", "content_type"),
    [
        ("swagger-ui-bundle.js", "javascript"),
        ("swagger-ui.css", "css"),
    ],
)
def test_the_vendored_assets_the_page_asks_for_are_served(
    client: TestClient, asset: str, content_type: str
):
    referenced = f"/v1/static/{asset}"
    assert referenced in client.get("/v1/docs").text

    response = client.get(referenced)

    assert response.status_code == 200
    assert content_type in response.headers["content-type"]
    assert len(response.content) > 10_000


def test_the_vendored_stylesheet_inlines_its_images(client: TestClient):
    # A swagger-ui build whose CSS fetched its icons instead of inlining them
    # would put the page back on the network without any of the above failing.
    stylesheet = client.get("/v1/static/swagger-ui.css").text

    fetched = [
        reference
        for reference in re.findall(r"url\(([^)]*)\)", stylesheet)
        if not reference.lstrip("\"'").startswith("data:")
    ]

    assert fetched == [], fetched
