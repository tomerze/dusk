from __future__ import annotations

import re

import pytest
from conftest import VIEWER_TOKEN, bearer
from starlette.testclient import TestClient


def page(client: TestClient) -> str:
    response = client.get("/v1/docs", headers=bearer(VIEWER_TOKEN))
    assert response.status_code == 200
    assert response.headers["content-type"].startswith("text/html")
    return response.text


def test_the_browsable_interface_is_served_over_the_document(client: TestClient):
    assert "/v1/openapi.json" in page(client)


def test_the_browsable_interface_needs_a_caller(client: TestClient):
    assert client.get("/v1/docs").status_code == 401


def test_the_browsable_interface_reaches_nothing_outside_dawn(client: TestClient):
    external = re.findall(r"""["'(](https?://[^"')\s]+)""", page(client))

    assert external == [], external


def test_the_swagger_validator_badge_is_turned_off(client: TestClient):
    text = page(client)

    assert '"validatorUrl": null' in text
    assert "validator.swagger.io" not in text


@pytest.mark.parametrize(
    ("asset", "content_type"),
    [("swagger-ui-bundle.js", "javascript"), ("swagger-ui.css", "css")],
)
def test_the_vendored_assets_the_page_asks_for_are_served(
    client: TestClient, asset: str, content_type: str
):
    referenced = f"/v1/static/{asset}"
    assert referenced in page(client)

    response = client.get(referenced, headers=bearer(VIEWER_TOKEN))

    assert response.status_code == 200
    assert content_type in response.headers["content-type"]
    assert len(response.content) > 10_000


def test_the_vendored_stylesheet_inlines_its_images(client: TestClient):
    stylesheet = client.get(
        "/v1/static/swagger-ui.css", headers=bearer(VIEWER_TOKEN)
    ).text

    fetched = [
        reference
        for reference in re.findall(r"url\(([^)]*)\)", stylesheet)
        if not reference.lstrip("\"'").startswith("data:")
    ]

    assert fetched == [], fetched
