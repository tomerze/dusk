from __future__ import annotations

import os
import secrets
from pathlib import Path

import pytest
from conftest import DISPATCHER_TOKEN, Dawn, bearer
from starlette.testclient import TestClient
from test_api import eventually

from dawn import binding

pytestmark = pytest.mark.integration

REQUIRED = (
    "DAWN_INTEGRATION_NIGHTFALL",
    "DAWN_INTEGRATION_CA",
    "DAWN_INTEGRATION_CERTIFICATE",
    "DAWN_INTEGRATION_KEY",
    "DAWN_INTEGRATION_NAMESPACE",
    "DAWN_INTEGRATION_DEVICE",
    "DAWN_INTEGRATION_INSTALLATION",
)
TLS_KEYWORDS = ("server_name", "ca", "certificate", "key")
RESULT_SECONDS = 120.0


def random_pid() -> str:
    return str(secrets.randbelow(2**63 - 2**16) + 2**16)


@pytest.fixture
def environment() -> dict[str, str]:
    missing = [name for name in REQUIRED if not os.environ.get(name)]
    if missing:
        pytest.skip(
            f"set {', '.join(missing)} to drive a real node through a real nightfall"
        )
    import dusk

    signature = getattr(getattr(dusk, "Dusk", None), "__text_signature__", None) or ""
    if not all(keyword in signature for keyword in TLS_KEYWORDS):
        pytest.skip(
            "the dusk extension does not take server_name, ca, certificate and key"
        )
    return {
        name: os.environ[name]
        for name in (*REQUIRED, "DAWN_INTEGRATION_SERVER_NAME_SUFFIX")
        if name in os.environ
    }


def nightfall_settings(environment: dict[str, str]) -> dict[str, object]:
    return {
        "default_inner_address": environment["DAWN_INTEGRATION_NIGHTFALL"],
        "server_name_suffix": environment.get(
            "DAWN_INTEGRATION_SERVER_NAME_SUFFIX", "fleet.dusk.example"
        ),
        "ca": environment["DAWN_INTEGRATION_CA"],
        "certificate": environment["DAWN_INTEGRATION_CERTIFICATE"],
        "key": environment["DAWN_INTEGRATION_KEY"],
    }


def test_dawn_reads_facts_runs_work_once_and_reaps_it_on_a_real_node(
    tmp_path: Path, environment: dict[str, str]
):
    dawn = Dawn(
        tmp_path,
        connector=binding.connect,
        nightfall=nightfall_settings(environment),
    )
    node = {
        "device_id": environment["DAWN_INTEGRATION_DEVICE"],
        "installation_id": environment["DAWN_INTEGRATION_INSTALLATION"],
        "namespace_id": environment["DAWN_INTEGRATION_NAMESPACE"],
        "nightfall": None,
    }
    pid = random_pid()
    work = {
        "pid": pid,
        "campaign_id": None,
        "attempt": None,
        "kind": "run_script",
        "script": "echo hello",
        "timeout_seconds": 60,
    }

    def statuses() -> list[str]:
        return [
            message["status"]
            for message in dawn.producer.results()
            if message["pid"] == pid
        ]

    def post(route: str, body: dict) -> None:
        response = client.post(route, json=body, headers=bearer(DISPATCHER_TOKEN))
        assert response.status_code == 202, response.text

    with TestClient(dawn.application, base_url="https://dawn-0.dawn:8443") as client:
        facts = client.post(
            "/v1/facts",
            json={"node": node, "pid": random_pid()},
            headers=bearer(DISPATCHER_TOKEN),
        )
        assert facts.status_code == 200, facts.text
        assert "dusk.version" in facts.json()["facts"]
        assert "dusk.device.id" not in facts.json()["facts"]

        for expected in (
            ["started", "succeeded"],
            ["started", "succeeded", "duplicate"],
        ):
            post("/v1/dispatch", {"node": node, "work": [work]})
            eventually(
                lambda expected=expected: statuses() == expected,
                seconds=RESULT_SECONDS,
            )

        post("/v1/reap", {"node": node, "pids": [pid]})
        eventually(lambda: statuses()[3:] == ["reaped"], seconds=RESULT_SECONDS)
        post("/v1/dispatch", {"node": node, "work": [work]})
        eventually(
            lambda: statuses()[4:] == ["started", "succeeded"], seconds=RESULT_SECONDS
        )
        post("/v1/reap", {"node": node, "pids": [pid]})
        eventually(lambda: statuses()[6:] == ["reaped"], seconds=RESULT_SECONDS)

    output = [
        message["value"]
        for message in dawn.producer.on("dusk.process-output")
        if message["pid"] == pid
    ]
    assert output == ["hello", "hello"]
