import os
import uuid

import psycopg
import pytest
from conftest import required, secret
from psycopg import sql


def connect(role: str, database: str = "inventory") -> psycopg.Connection:
    return psycopg.connect(
        host=required("POSTGRES_HOST"),
        dbname=database,
        user=role,
        password=secret(f"postgres-{role}", "password"),
        connect_timeout=10,
        autocommit=True,
    )


REPORTING_TABLES = {
    "nodes",
    "node_presence",
    "campaigns",
    "campaign_counters",
    "campaign_events",
    "campaign_nodes",
    "alerts",
    "ledger_chain_heads",
}


def test_twilight_owns_the_inventory_database() -> None:
    with connect("twilight") as twilight:
        owner = twilight.execute(
            "SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname = 'inventory'"
        ).fetchone()
    assert owner == ("twilight",)


def test_grafana_may_read_exactly_the_reporting_tables() -> None:
    with connect("twilight") as twilight, twilight.transaction(force_rollback=True):
        existing = {
            name
            for (name,) in twilight.execute(
                "SELECT tablename FROM pg_tables WHERE schemaname = 'public'"
            )
        }
        for name in sorted(REPORTING_TABLES - existing) + [
            f"verify_{uuid.uuid4().hex}"
        ]:
            twilight.execute(
                sql.SQL("CREATE TABLE {} (value integer)").format(sql.Identifier(name))
            )
        readable = dict(
            twilight.execute(
                "SELECT tablename, has_table_privilege('grafana', format('public.%I', tablename), 'SELECT') "
                "FROM pg_tables WHERE schemaname = 'public'"
            ).fetchall()
        )
    assert {name for name, allowed in readable.items() if allowed} == REPORTING_TABLES


def test_grafana_can_neither_write_nor_create() -> None:
    with connect("grafana") as grafana:
        assert grafana.execute("SHOW default_transaction_read_only").fetchone() == (
            "on",
        )
        grafana.execute("SET default_transaction_read_only = off")
        with pytest.raises(psycopg.errors.InsufficientPrivilege):
            grafana.execute("CREATE TABLE public.verify_forbidden (value integer)")
        with pytest.raises(psycopg.errors.InsufficientPrivilege):
            grafana.execute("CREATE TEMPORARY TABLE verify_forbidden (value integer)")
        privileges = grafana.execute(
            "SELECT count(*) FROM information_schema.table_privileges "
            "WHERE grantee = 'grafana' AND privilege_type <> 'SELECT'"
        ).fetchone()
    assert privileges == (0,)


def test_signoz_has_its_own_database_and_no_inventory_access() -> None:
    if not os.path.exists("/secrets/postgres-signoz"):
        pytest.skip("the signoz password is not mounted")
    with connect("signoz", "signoz") as signoz:
        assert signoz.execute("SELECT current_database()").fetchone() == ("signoz",)
    with pytest.raises(psycopg.OperationalError):
        connect("signoz", "inventory")
