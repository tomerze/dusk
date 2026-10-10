import datetime
from collections.abc import Callable
from typing import Any

import pytest
from conftest import insert_chains

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
    assert engines == expected


def test_every_table_expires_its_rows(
    clickhouse: Callable[[str], list[dict[str, Any]]],
) -> None:
    for table in TABLES:
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
