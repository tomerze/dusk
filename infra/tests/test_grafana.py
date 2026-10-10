import base64
import datetime
import functools
import json
import urllib.error
import urllib.request
from collections.abc import Callable
from pathlib import Path
from typing import Any

import psycopg
import pytest
from conftest import insert_chains, required, secret

DASHBOARDS = Path(__file__).resolve().parents[1] / "grafana" / "dashboards"
ALERT_RULES = {
    "dusk-open-critical-alert",
    "dusk-ledger-chain-broken",
    "dusk-ledger-evidence-stalled",
    "dusk-contract-messages-dropped",
    "dusk-alert-delivery-failing",
}
RECEIVERS = {
    "on-call": "pagerduty",
    "chat": "slack",
    "noc": "teams",
    "siem": "webhook",
    "mail": "email",
}


def grafana(path: str, body: Any = None) -> Any:
    password = Path(required("GRAFANA_PASSWORD_FILE")).read_text().strip()
    request = urllib.request.Request(
        required("GRAFANA_URL") + path,
        data=None if body is None else json.dumps(body).encode(),
        headers={
            "Authorization": "Basic "
            + base64.b64encode(f"admin:{password}".encode()).decode(),
            "Content-Type": "application/json",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            return json.load(response)
    except urllib.error.HTTPError as failure:
        raise AssertionError(
            f"{path}: {failure.code} {failure.read().decode()}"
        ) from failure


@functools.cache
def twilight_has_migrated() -> bool:
    with psycopg.connect(
        host=required("POSTGRES_HOST"),
        dbname="inventory",
        user="grafana",
        password=secret("postgres-grafana", "password"),
        connect_timeout=10,
    ) as connection:
        found = connection.execute("SELECT to_regclass('public.nodes')").fetchone()
    return found is not None and found[0] is not None


def skip_without_inventory(datasource: str) -> None:
    if datasource == "dusk-inventory" and not twilight_has_migrated():
        pytest.skip(
            "twilight's migrations have not run: the inventory database has no nodes table"
        )


def alert_rules() -> dict[str, Any]:
    return {rule["uid"]: rule for rule in grafana("/api/v1/provisioning/alert-rules")}


def panel_queries() -> list[tuple[str, str, dict[str, Any]]]:
    queries = []
    for path in sorted(DASHBOARDS.glob("*.json")):
        for panel in json.loads(path.read_text())["panels"]:
            for target in panel.get("targets", []):
                queries.append((path.stem, panel["title"], target))
    return queries


@pytest.mark.parametrize("uid", ["dusk-inventory", "dusk-clickhouse"])
def test_datasource_is_healthy(uid: str) -> None:
    health = grafana(f"/api/datasources/uid/{uid}/health")
    assert health["status"] == "OK", health


def test_every_dashboard_is_provisioned_in_the_dusk_folder() -> None:
    found = grafana("/api/search?type=dash-db&tag=dusk")
    expected = {
        json.loads(path.read_text())["uid"] for path in DASHBOARDS.glob("*.json")
    }
    assert {item["uid"] for item in found} == expected
    assert {item["folderTitle"] for item in found} == {"Dusk"}


@pytest.mark.parametrize(
    ("dashboard", "title", "target"),
    panel_queries(),
    ids=[f"{dashboard}: {title}" for dashboard, title, _ in panel_queries()],
)
def test_every_panel_query_runs(
    dashboard: str, title: str, target: dict[str, Any]
) -> None:
    skip_without_inventory(target["datasource"]["uid"])
    query = dict(target)
    query["rawSql"] = query["rawSql"].replace("${metric}", "up")
    response = grafana(
        "/api/ds/query",
        {
            "from": "now-1h",
            "to": "now",
            "queries": [query | {"intervalMs": 60000, "maxDataPoints": 500}],
        },
    )
    result = response["results"]["A"]
    assert "error" not in result, result.get("error")


def test_every_alert_rule_is_provisioned() -> None:
    assert set(alert_rules()) == ALERT_RULES


def test_every_alert_rule_names_its_runbook_kind() -> None:
    for uid, rule in alert_rules().items():
        assert rule["labels"].get("kind"), uid
        assert rule["labels"].get("severity") in {
            "critical",
            "high",
            "medium",
            "low",
        }, uid


def test_contact_points_follow_twilights_receivers() -> None:
    points = grafana("/api/v1/provisioning/contact-points")
    provisioned = {
        point["name"]: point["type"]
        for point in points
        if point["uid"].startswith("dusk-")
    }
    assert provisioned == RECEIVERS
    policy = grafana("/api/v1/provisioning/policies")
    assert {route["receiver"] for route in policy["routes"]} == set(RECEIVERS)
    assert all(route.get("continue") for route in policy["routes"])
    templates = {
        template["name"]: template["template"]
        for template in grafana("/api/v1/provisioning/templates")
    }
    assert "/stack/runbooks/{{ . }}/" in templates["dusk"]


@pytest.mark.parametrize("uid", sorted(ALERT_RULES))
def test_alert_rule_query_runs(uid: str) -> None:
    rule = alert_rules()[uid]
    query = next(item for item in rule["data"] if item["refId"] == "A")
    skip_without_inventory(query["datasourceUid"])
    model = query["model"] | {"datasource": {"uid": query["datasourceUid"]}}
    response = grafana(
        "/api/ds/query", {"from": "now-15m", "to": "now", "queries": [model]}
    )
    assert "error" not in response["results"]["A"], response["results"]["A"].get(
        "error"
    )
    assert (
        "{{ $values.B }}" in rule["annotations"]["summary"]
        or "{{" not in rule["annotations"]["summary"]
    )


def rule_query(uid: str, database: str) -> str:
    rule = alert_rules()[uid]
    query = next(item for item in rule["data"] if item["refId"] == "A")
    return query["model"]["rawSql"].replace("dusk.", f"{database}.")


def evidence_count(instance: str, seconds_ago: int, value: int) -> str:
    return (
        f"(now64(9) - INTERVAL {seconds_ago} SECOND, 'vector', "
        f"'vector_component_sent_events_total', {value}, "
        f"map('component_id', 'evidence_ledger'), map('service.instance.id', '{instance}'))"
    )


def test_the_evidence_alert_fires_while_every_vector_instance_is_stalled(
    clickhouse: Callable[[str], list[dict[str, Any]]], scratch: str
) -> None:
    clickhouse(
        f"INSERT INTO {scratch}.ledger (time, instance, sequence, previous_hash, hash, kind) "
        "VALUES (now64(9) - INTERVAL 7 MINUTE, 'verify', 0, 'start', 'a0', 'call')"
    )
    insert = (
        f"INSERT INTO {scratch}.otel_metrics "
        "(time, service_name, metric_name, value, attributes, resource_attributes) VALUES "
    )
    clickhouse(
        insert
        + ", ".join(
            [
                evidence_count("vector-a", 240, 100),
                evidence_count("vector-a", 60, 100),
                evidence_count("vector-b", 240, 200),
                evidence_count("vector-b", 60, 200),
            ]
        )
    )
    query = rule_query("dusk-ledger-evidence-stalled", scratch)
    assert int(clickhouse(query)[0]["stalled"]) == 1
    clickhouse(insert + evidence_count("vector-b", 30, 250))
    assert int(clickhouse(query)[0]["stalled"]) == 0


def test_the_chain_alert_counts_each_broken_chain(
    clickhouse: Callable[[str], list[dict[str, Any]]], scratch: str
) -> None:
    moment = datetime.datetime.now(datetime.UTC) - datetime.timedelta(minutes=30)
    insert_chains(
        clickhouse,
        scratch,
        {
            "intact": [(0, "start", "a0"), (1, "a0", "a1")],
            "broken": [(0, "start", "c0"), (1, "forged", "c1")],
        },
        moment.strftime("%Y-%m-%d %H:%M:%S"),
    )
    query = rule_query("dusk-ledger-chain-broken", scratch)
    assert int(clickhouse(query)[0]["broken"]) == 1
