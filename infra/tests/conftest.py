import base64
import json
import os
import socket
import time
import urllib.parse
import urllib.request
import uuid
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import boto3
import pytest

SECRETS = Path(os.environ.get("SECRETS_DIRECTORY", "/secrets"))
VERIFY_PROJECT = "dusk-verify"
SERVICE_VARIABLES = (
    "STEP_CA_URL",
    "KAFKA_BROKERS",
    "CLICKHOUSE_URL",
    "S3_ENDPOINT",
    "OTEL_COLLECTOR_URL",
    "POSTGRES_HOST",
    "GRAFANA_URL",
)
STACK_SERVICES = ("nightfall", "dawn", "twilight")


def resolves(name: str) -> bool:
    try:
        socket.getaddrinfo(name, None)
    except socket.gaierror:
        return False
    return True


@pytest.fixture(scope="session", autouse=True)
def throwaway_project() -> None:
    if not any(os.environ.get(variable) for variable in SERVICE_VARIABLES):
        return
    project = os.environ.get("COMPOSE_PROJECT_NAME", "")
    if project != VERIFY_PROJECT:
        pytest.exit(
            "these tests write test data into Kafka, ClickHouse, the object store and "
            f"step-ca: run them in the compose project {VERIFY_PROJECT}, not "
            f"{project or 'an unnamed project'}",
            returncode=2,
        )
    running = [name for name in STACK_SERVICES if resolves(name)]
    if running:
        pytest.exit(
            f"{', '.join(running)} run in the project {VERIFY_PROJECT} and would "
            "read the test data: run only the dependency tier there",
            returncode=2,
        )


def secret(group: str, name: str) -> str:
    return (SECRETS / group / name).read_text().strip()


def required(variable: str) -> str:
    value = os.environ.get(variable, "")
    if not value:
        pytest.skip(f"{variable} names the service under test")
    return value


def eventually[Result](
    probe: Callable[[], Result | None], seconds: float, what: str
) -> Result:
    deadline = time.monotonic() + seconds
    delay = 0.5
    while True:
        result = probe()
        if result is not None:
            return result
        if time.monotonic() > deadline:
            raise AssertionError(f"{what} did not happen within {seconds} seconds")
        time.sleep(delay)
        delay = min(delay * 2, 5.0)


@pytest.fixture(scope="session")
def clickhouse() -> Callable[[str], list[dict[str, Any]]]:
    url = required("CLICKHOUSE_URL")
    password = Path(required("CLICKHOUSE_PASSWORD_FILE")).read_text().strip()
    authorization = "Basic " + base64.b64encode(f"default:{password}".encode()).decode()

    def query(sql: str) -> list[dict[str, Any]]:
        request = urllib.request.Request(
            f"{url}/?{urllib.parse.urlencode({'default_format': 'JSONEachRow'})}",
            data=sql.encode(),
            headers={"Authorization": authorization},
        )
        with urllib.request.urlopen(request, timeout=30) as response:
            return [
                json.loads(line)
                for line in response.read().decode().splitlines()
                if line
            ]

    return query


@pytest.fixture
def scratch(clickhouse: Callable[[str], list[dict[str, Any]]]) -> Iterator[str]:
    database = f"verify_{uuid.uuid4().hex}"
    clickhouse(f"CREATE DATABASE {database}")
    try:
        for table in ("ledger", "otel_metrics"):
            clickhouse(
                f"CREATE TABLE {database}.{table} AS dusk.{table} "
                "ENGINE = MergeTree ORDER BY tuple()"
            )
        view = clickhouse(
            "SELECT create_table_query FROM system.tables "
            "WHERE database = 'dusk' AND name = 'ledger_chain'"
        )[0]["create_table_query"]
        clickhouse(view.replace("dusk.", f"{database}."))
        yield database
    finally:
        clickhouse(f"DROP DATABASE {database} SYNC")


def insert_chains(
    clickhouse: Callable[[str], list[dict[str, Any]]],
    database: str,
    chains: dict[str, list[tuple[int, str, str]]],
    moment: str,
) -> None:
    rows = ", ".join(
        f"('{moment}', '{instance}', {sequence}, '{previous_hash}', '{entry_hash}', 'call')"
        for instance, entries in chains.items()
        for sequence, previous_hash, entry_hash in entries
    )
    clickhouse(
        f"INSERT INTO {database}.ledger (time, instance, sequence, previous_hash, hash, kind) VALUES {rows}"
    )


@pytest.fixture(scope="session")
def object_store() -> Callable[[str], Any]:
    endpoint = required("S3_ENDPOINT")

    def client(user: str) -> Any:
        return boto3.client(
            "s3",
            endpoint_url=endpoint,
            region_name="us-east-1",
            aws_access_key_id=secret(f"ceph-{user}", "access-key"),
            aws_secret_access_key=secret(f"ceph-{user}", "secret-key"),
        )

    return client
