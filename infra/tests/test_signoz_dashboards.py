import json
import re
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[1]
DASHBOARDS = ROOT / "k8s" / "base" / "signoz" / "dashboards"
SCHEMA = ROOT / "k8s" / "base" / "clickhouse" / "schema"
KEYWORDS = {
    "AND",
    "ARRAY",
    "AS",
    "ASC",
    "BY",
    "DESC",
    "FROM",
    "GROUP",
    "IN",
    "IS",
    "JOIN",
    "LIMIT",
    "NOT",
    "NULL",
    "OR",
    "ORDER",
    "SELECT",
    "WHERE",
}
STRING = re.compile(r"'(?:[^'\\]|\\.)*'")
VARIABLE = re.compile(r"\$\w+")


def top_level_split(text: str) -> list[str]:
    parts = [""]
    depth = 0
    for character in text:
        depth += character == "("
        depth -= character == ")"
        if character == "," and depth == 0:
            parts.append("")
        else:
            parts[-1] += character
    return [part.strip() for part in parts]


def parenthesized(text: str) -> str:
    start = text.index("(")
    depth = 0
    for index in range(start, len(text)):
        depth += text[index] == "("
        depth -= text[index] == ")"
        if depth == 0:
            return text[start + 1 : index]
    raise AssertionError(f"unbalanced parentheses in {text[:80]}")


def last_select_list(statement: str) -> tuple[list[str], str]:
    depth = 0
    start = -1
    for index, character in enumerate(statement):
        depth += character == "("
        depth -= character == ")"
        if depth == 0 and statement.startswith("SELECT", index):
            start = index + len("SELECT")
    tail = statement[start:]
    depth = 0
    for index, character in enumerate(tail):
        depth += character == "("
        depth -= character == ")"
        if depth == 0 and tail.startswith("FROM", index):
            source = re.match(r"FROM\s+(?:dusk\.)?(\w+)", tail[index:])
            assert source is not None
            return top_level_split(tail[:index]), source[1]
    raise AssertionError(f"no FROM after the last SELECT in {statement[:80]}")


def schema() -> tuple[dict[str, set[str]], dict[str, set[str]]]:
    columns: dict[str, set[str]] = {}
    parameters: dict[str, set[str]] = {}
    for path in sorted(SCHEMA.glob("*.sql")):
        for statement in STRING.sub("''", path.read_text()).split(";"):
            created = re.match(
                r"\s*CREATE (TABLE IF NOT EXISTS|OR REPLACE VIEW) dusk\.(\w+)",
                statement,
            )
            if created is None:
                continue
            kind, name = created.groups()
            copied = re.search(r"ON CLUSTER dusk AS dusk\.(\w+)", statement)
            if kind.startswith("TABLE") and copied:
                columns[name] = set(columns[copied[1]])
            elif kind.startswith("TABLE"):
                columns[name] = {
                    line.split()[0].strip("`")
                    for line in top_level_split(parenthesized(statement))
                    if not re.match(r"(INDEX|PROJECTION|CONSTRAINT)\b", line)
                }
            else:
                items, source = last_select_list(statement)
                names: set[str] = set()
                for item in items:
                    alias = re.search(r"\bAS\s+(\w+)$", item)
                    if item == "*":
                        names |= columns[source]
                    elif alias:
                        names.add(alias[1])
                    else:
                        assert re.fullmatch(r"\w+", item), item
                        names.add(item)
                columns[name] = names
                parameters[name] = set(re.findall(r"\{(\w+):", statement))
    return columns, parameters


def panels() -> list[tuple[str, str, str, str]]:
    found = []
    for path in sorted(DASHBOARDS.glob("*.json")):
        spec = json.loads(path.read_text())["spec"]
        for key, panel in spec["panels"].items():
            for query in panel["spec"]["queries"]:
                plugin = query["spec"]["plugin"]
                assert plugin["kind"] == "signoz/CompositeQuery", (path.name, key)
                for envelope in plugin["spec"]["queries"]:
                    assert envelope["type"] == "clickhouse_sql", (path.name, key)
                    found.append(
                        (path.stem, key, query["kind"], envelope["spec"]["query"])
                    )
    return found


def test_every_dashboard_parses_and_lays_out_each_of_its_panels() -> None:
    paths = sorted(DASHBOARDS.glob("*.json"))
    assert {path.stem for path in paths} == {
        "connections",
        "enrollments",
        "ledger",
        "process-results",
    }
    for path in paths:
        assert re.fullmatch(r"[a-z0-9-]+", path.stem)
        exported = json.loads(path.read_text())
        assert set(exported) == {"spec", "tags", "image"}
        spec = exported["spec"]
        laid_out = [
            item["content"]["$ref"].removeprefix("#/spec/panels/")
            for layout in spec["layouts"]
            for item in layout["spec"]["items"]
        ]
        assert sorted(laid_out) == sorted(spec["panels"]), path.name


@pytest.mark.parametrize(
    ("dashboard", "panel", "kind", "sql"),
    panels(),
    ids=[f"{dashboard}: {panel}" for dashboard, panel, _, _ in panels()],
)
def test_every_panel_names_only_tables_and_columns_of_the_schema(
    dashboard: str, panel: str, kind: str, sql: str
) -> None:
    columns, parameters = schema()
    text = VARIABLE.sub("0", STRING.sub("''", sql))
    tables = re.findall(r"\bdusk\.(\w+)", text)
    assert tables, sql
    assert set(tables) <= set(columns), sorted(set(tables) - set(columns))
    known = set().union(
        *(columns[table] | parameters.get(table, set()) for table in tables)
    )
    aliases = set(re.findall(r"\bAS\s+(\w+)", text))
    text = re.sub(r"\bdusk\.\w+", " ", text)
    unknown = {
        identifier
        for identifier in re.findall(r"(?<![\w.])([A-Za-z_]\w*)\b(?!\s*\()", text)
        if identifier.upper() not in KEYWORDS
        and identifier not in aliases
        and identifier not in known
    }
    assert not unknown, f"{sorted(unknown)} are not columns of {sorted(set(tables))}"


@pytest.mark.parametrize(
    ("dashboard", "panel", "kind", "sql"),
    panels(),
    ids=[f"{dashboard}: {panel}" for dashboard, panel, _, _ in panels()],
)
def test_every_panel_query_runs(
    clickhouse: Callable[[str], list[dict[str, Any]]],
    dashboard: str,
    panel: str,
    kind: str,
    sql: str,
) -> None:
    end = int(time.time())
    start = end - 3600
    values = {
        "start_datetime": f"toDateTime({start})",
        "end_datetime": f"toDateTime({end})",
        "start_timestamp": str(start),
        "end_timestamp": str(end),
    }
    for name in sorted(values, key=len, reverse=True):
        sql = sql.replace(f"${name}", values[name])
    clickhouse(sql)
