import io
import json
import os
import time
import urllib.request
import uuid
from collections.abc import Callable
from compression import zstd
from pathlib import Path
from typing import Any

import pyarrow.parquet
import pytest
from confluent_kafka import Producer
from conftest import eventually, required

TABLES = {
    "dusk.ledger": "ledger",
    "dusk.connections": "connections",
    "dusk.enrollments": "enrollments",
    "dusk.process-results": "process_results",
    "dusk.process-output": "process_output",
    "dusk.files": "files",
}
DELIVERY_SECONDS = 180
LAKE_SECONDS = 420


def fixtures() -> list[tuple[str, bool, dict[str, Any]]]:
    contracts = Path(required("CONTRACTS_DIRECTORY"))
    return [
        (topic, example.name.startswith("invalid-"), json.loads(example.read_text()))
        for topic in TABLES
        for example in sorted((contracts / "examples" / topic).glob("*.json"))
    ]


@pytest.fixture(scope="module")
def produced() -> dict[str, Any]:
    producer = Producer(
        {
            "bootstrap.servers": required("KAFKA_BROKERS"),
            "allow.auto.create.topics": "false",
            "enable.idempotence": "true",
            "acks": "all",
        }
    )
    valid: dict[str, list[str]] = {topic: [] for topic in TABLES}
    invalid: list[str] = []
    ledger_lines: list[bytes] = []
    for topic, malformed, message in fixtures():
        if malformed:
            invalid.append(str(message["id"]))
        else:
            message["id"] = str(uuid.uuid7())
            valid[topic].append(message["id"])
        line = json.dumps(message, separators=(",", ":")).encode()
        if topic == "dusk.ledger" and not malformed:
            ledger_lines.append(line)
        producer.produce(topic, value=line, key=str(message["id"]).encode())
    remaining = producer.flush(30)
    assert remaining == 0
    return {
        "valid": valid,
        "invalid": invalid,
        "ledger_lines": ledger_lines,
        "started": time.time(),
    }


def test_every_valid_message_reaches_its_clickhouse_table(
    produced: dict[str, Any], clickhouse: Callable[[str], list[dict[str, Any]]]
) -> None:
    for topic, identifiers in produced["valid"].items():
        table = TABLES[topic]
        quoted = ", ".join(f"'{identifier}'" for identifier in identifiers)

        def arrived(
            table: str = table, quoted: str = quoted, expected: int = len(identifiers)
        ) -> int | None:
            count = clickhouse(
                f"SELECT count() AS rows FROM dusk.{table} FINAL WHERE id IN ({quoted})"
            )[0]["rows"]
            return count if int(count) == expected else None

        eventually(
            arrived, DELIVERY_SECONDS, f"{len(identifiers)} rows in dusk.{table}"
        )


def test_no_invalid_message_reaches_clickhouse(
    produced: dict[str, Any], clickhouse: Callable[[str], list[dict[str, Any]]]
) -> None:
    test_every_valid_message_reaches_its_clickhouse_table(produced, clickhouse)
    quoted = ", ".join(f"'{identifier}'" for identifier in produced["invalid"])
    for table in TABLES.values():
        rows = clickhouse(
            f"SELECT toString(id) AS id FROM dusk.{table} WHERE toString(id) IN ({quoted})"
        )
        assert rows == [], table


def test_ledger_entries_reach_the_evidence_bucket_byte_for_byte(
    produced: dict[str, Any], object_store: Callable[[str], Any]
) -> None:
    admin = object_store("admin")
    wanted = set(produced["ledger_lines"])

    def stored() -> set[bytes] | None:
        found: set[bytes] = set()
        for page in admin.get_paginator("list_objects_v2").paginate(
            Bucket="dusk-ledger-evidence", Prefix="ledger/"
        ):
            for item in page.get("Contents", []):
                if item["LastModified"].timestamp() < produced["started"] - 5:
                    continue
                body = admin.get_object(Bucket="dusk-ledger-evidence", Key=item["Key"])[
                    "Body"
                ].read()
                found.update(zstd.decompress(body).splitlines())
        return found if wanted <= found else None

    eventually(stored, DELIVERY_SECONDS, "every ledger entry in the evidence bucket")


def test_otlp_signals_reach_their_clickhouse_tables(
    clickhouse: Callable[[str], list[dict[str, Any]]],
) -> None:
    collector = required("OTEL_COLLECTOR_URL")
    marker = str(uuid.uuid4())
    now = time.time_ns()
    resource = {
        "attributes": [{"key": "service.name", "value": {"stringValue": "verify"}}]
    }
    attributes = [{"key": "marker", "value": {"stringValue": marker}}]
    signals = {
        "logs": {
            "resourceLogs": [
                {
                    "resource": resource,
                    "scopeLogs": [
                        {
                            "scope": {"name": "verify"},
                            "logRecords": [
                                {
                                    "timeUnixNano": str(now),
                                    "severityNumber": 9,
                                    "severityText": "INFO",
                                    "body": {"stringValue": marker},
                                    "attributes": attributes,
                                }
                            ],
                        }
                    ],
                }
            ]
        },
        "traces": {
            "resourceSpans": [
                {
                    "resource": resource,
                    "scopeSpans": [
                        {
                            "scope": {"name": "verify"},
                            "spans": [
                                {
                                    "traceId": uuid.uuid4().hex,
                                    "spanId": uuid.uuid4().hex[:16],
                                    "name": marker,
                                    "kind": 2,
                                    "startTimeUnixNano": str(now),
                                    "endTimeUnixNano": str(now + 1000),
                                    "attributes": attributes,
                                }
                            ],
                        }
                    ],
                }
            ]
        },
        "metrics": {
            "resourceMetrics": [
                {
                    "resource": resource,
                    "scopeMetrics": [
                        {
                            "scope": {"name": "verify"},
                            "metrics": [
                                {
                                    "name": "verify_marker",
                                    "gauge": {
                                        "dataPoints": [
                                            {
                                                "timeUnixNano": str(now),
                                                "asInt": "7",
                                                "attributes": attributes,
                                            }
                                        ]
                                    },
                                }
                            ],
                        }
                    ],
                }
            ]
        },
    }
    for signal, body in signals.items():
        request = urllib.request.Request(
            f"{collector}/v1/{signal}",
            data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=10) as response:
            assert response.status == 200
    checks = {
        "otel_logs": f"SELECT count() AS rows FROM dusk.otel_logs WHERE body = '{marker}' AND service_name = 'verify'",
        "otel_spans": f"SELECT count() AS rows FROM dusk.otel_spans WHERE name = '{marker}' AND kind = 'SPAN_KIND_SERVER'",
        "otel_metrics": (
            "SELECT count() AS rows FROM dusk.otel_metrics "
            f"WHERE attributes['marker'] = '{marker}' AND value = 7 AND metric_type = 'gauge'"
        ),
    }
    for table, sql in checks.items():
        eventually(
            lambda sql=sql: True if int(clickhouse(sql)[0]["rows"]) == 1 else None,
            DELIVERY_SECONDS,
            f"the marker in dusk.{table}",
        )


@pytest.mark.skipif(
    not os.environ.get("VERIFY_LAKE"),
    reason="VERIFY_LAKE=1 waits for the lake's 300 s batches",
)
def test_every_valid_message_reaches_the_lake_and_its_view(
    produced: dict[str, Any],
    object_store: Callable[[str], Any],
    clickhouse: Callable[[str], list[dict[str, Any]]],
) -> None:
    reader = object_store("reader")
    for topic, identifiers in produced["valid"].items():
        table = TABLES[topic]

        def written(
            table: str = table, identifiers: list[str] = identifiers
        ) -> bool | None:
            found: set[str] = set()
            for page in reader.get_paginator("list_objects_v2").paginate(
                Bucket="dusk-lake", Prefix=f"{table}/dt="
            ):
                for item in page.get("Contents", []):
                    body = reader.get_object(Bucket="dusk-lake", Key=item["Key"])[
                        "Body"
                    ].read()
                    found.update(
                        pyarrow.parquet.read_table(io.BytesIO(body), columns=["id"])
                        .column("id")
                        .to_pylist()
                    )
            return True if set(identifiers) <= found else None

        eventually(written, LAKE_SECONDS, f"every {table} message in the lake")
        quoted = ", ".join(f"'{identifier}'" for identifier in identifiers)
        rows = clickhouse(
            f"SELECT count(DISTINCT id) AS rows, min(dt) AS day FROM dusk.lake_{table} WHERE id IN ({quoted})"
        )
        assert int(rows[0]["rows"]) == len(identifiers)
