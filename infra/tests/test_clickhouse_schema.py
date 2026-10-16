import base64
import datetime
import urllib.error
import urllib.request
import uuid
from collections.abc import Callable
from typing import Any

import pytest
from conftest import insert_chains, required, secret

TOPIC_TABLES = [
    "ledger",
    "connections",
    "enrollments",
    "process_results",
    "process_output",
    "files",
]
OTEL_TABLES = ["otel_logs", "otel_spans", "otel_metrics"]
TABLES = TOPIC_TABLES + OTEL_TABLES


def test_the_dusk_database_holds_every_table_view_and_lake_view(
    clickhouse: Callable[[str], list[dict[str, Any]]],
) -> None:
    rows = clickhouse(
        "SELECT name, engine FROM system.tables WHERE database = 'dusk' ORDER BY name"
    )
    engines = {row["name"]: row["engine"] for row in rows}
    expected = {table: "ReplicatedReplacingMergeTree" for table in TOPIC_TABLES}
    expected |= {table: "ReplicatedMergeTree" for table in OTEL_TABLES}
    expected |= {f"lake_{table}_parquet": "S3" for table in TABLES}
    expected |= {f"lake_{table}": "View" for table in TABLES}
    expected["ledger_chain"] = "View"
    expected["erasure_scope"] = "ReplicatedReplacingMergeTree"
    assert engines == expected


def test_every_table_expires_its_rows(
    clickhouse: Callable[[str], list[dict[str, Any]]],
) -> None:
    for table in TABLES + ["erasure_scope"]:
        create = clickhouse(f"SHOW CREATE TABLE dusk.{table}")[0]["statement"]
        assert "TTL" in create, table


def test_the_ledger_is_partitioned_by_month_and_ordered_by_identity_and_time(
    clickhouse: Callable[[str], list[dict[str, Any]]],
) -> None:
    row = clickhouse(
        "SELECT partition_key, sorting_key FROM system.tables WHERE database = 'dusk' AND name = 'ledger'"
    )[0]
    assert row["partition_key"] == "toYYYYMM(time)"
    assert row["sorting_key"].startswith("device_id, installation_id, time")


def test_the_process_output_is_ordered_by_campaign_pid_and_index(
    clickhouse: Callable[[str], list[dict[str, Any]]],
) -> None:
    row = clickhouse(
        "SELECT sorting_key FROM system.tables WHERE database = 'dusk' AND name = 'process_output'"
    )[0]
    assert row["sorting_key"] == "campaign_id, pid, index"


@pytest.mark.parametrize(
    "table", ["ledger", "process_results", "process_output", "files"]
)
def test_a_pid_is_a_u64_column(
    clickhouse: Callable[[str], list[dict[str, Any]]], table: str
) -> None:
    row = clickhouse(
        f"SELECT type FROM system.columns WHERE database = 'dusk' AND table = '{table}' AND name = 'pid'"
    )[0]
    assert row["type"] == "UInt64"


CHAINS = {
    "intact": [(0, "start", "a0"), (1, "a0", "a1"), (2, "a1", "a2")],
    "gap": [(0, "start", "b0"), (1, "b0", "b1"), (3, "b2", "b3")],
    "broken": [(0, "start", "c0"), (1, "forged", "c1")],
    "conflict": [(0, "start", "d0"), (1, "d0", "d1"), (1, "d0", "d2")],
    "duplicate": [(0, "start", "e0"), (1, "e0", "e1"), (1, "e0", "e1")],
}


def test_the_ledger_chain_view_tells_every_kind_of_break_apart(
    clickhouse: Callable[[str], list[dict[str, Any]]], scratch: str
) -> None:
    moment = datetime.datetime.now(datetime.UTC) - datetime.timedelta(hours=1)
    insert_chains(clickhouse, scratch, CHAINS, moment.strftime("%Y-%m-%d %H:%M:%S"))
    rows = clickhouse(
        "SELECT instance, entries, gaps, broken_links, conflicting_entries, intact "
        f"FROM {scratch}.ledger_chain(since = '2000-01-01 00:00:00')"
    )
    found = {
        row["instance"]: tuple(
            int(row[column])
            for column in [
                "entries",
                "gaps",
                "broken_links",
                "conflicting_entries",
                "intact",
            ]
        )
        for row in rows
    }
    assert found == {
        "intact": (3, 0, 0, 0, 1),
        "gap": (3, 1, 0, 0, 0),
        "broken": (2, 0, 1, 0, 0),
        "conflict": (3, 0, 0, 1, 0),
        "duplicate": (2, 0, 0, 0, 1),
    }


def test_twilight_may_erase_rows_but_not_change_the_ledger(
    clickhouse: Callable[[str], list[dict[str, Any]]],
) -> None:
    password = secret("clickhouse-twilight", "password")
    authorization = (
        "Basic " + base64.b64encode(f"twilight:{password}".encode()).decode()
    )

    def as_twilight(sql: str) -> int:
        request = urllib.request.Request(
            required("CLICKHOUSE_URL") + "/",
            data=sql.encode(),
            headers={"Authorization": authorization},
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                response.read()
                return response.status
        except urllib.error.HTTPError as failure:
            return failure.code

    erasure = str(uuid.uuid4())
    assert (
        as_twilight(
            f"INSERT INTO dusk.erasure_scope (erasure_id, kind, value) VALUES ('{erasure}', 'device', 'verify')"
        )
        == 200
    )
    assert as_twilight("SELECT count() FROM dusk.connections") == 200
    assert (
        as_twilight(
            f"SELECT count() FROM dusk.lake_connections_parquet WHERE _path = 'verify-{erasure}'"
        )
        == 200
    )
    assert (
        as_twilight(
            f"SELECT count() FROM s3(lake, filename = 'verify-{erasure}.parquet', format = 'Parquet', structure = 'a UInt8') "
            "SETTINGS s3_ignore_file_doesnt_exist = 1"
        )
        == 200
    )
    assert as_twilight("SYSTEM SYNC REPLICA dusk.erasure_scope") == 200
    assert (
        as_twilight(
            "ALTER TABLE dusk.files DELETE WHERE device_id IN "
            f"(SELECT value FROM dusk.erasure_scope WHERE erasure_id = '{erasure}') SETTINGS allow_nondeterministic_mutations = 1"
        )
        == 200
    )
    assert as_twilight("INSERT INTO dusk.ledger (instance) VALUES ('forged')") != 200
    assert as_twilight("TRUNCATE TABLE dusk.ledger") != 200
    assert as_twilight("DROP TABLE dusk.erasure_scope") != 200
    clickhouse(f"ALTER TABLE dusk.erasure_scope DELETE WHERE erasure_id = '{erasure}'")
