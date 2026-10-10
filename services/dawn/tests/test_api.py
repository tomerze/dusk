from __future__ import annotations

import os
import time

import pytest
from conftest import (
    DEFAULT_SH_PID,
    DEVICE_ID,
    DISPATCHER_TOKEN,
    INSTALLATION_ID,
    NAMESPACE_ID,
    OPERATOR_TOKEN,
    PID,
    SECOND_OPERATOR_TOKEN,
    VIEWER_TOKEN,
    Dawn,
    FakeNode,
    Wait,
    bearer,
)
from starlette.testclient import TestClient

NODE = {
    "device_id": DEVICE_ID,
    "installation_id": INSTALLATION_ID,
    "namespace_id": NAMESPACE_ID,
    "nightfall": None,
}


def pid(number: int) -> str:
    return str(0x10000 + number)


def eventually(condition, seconds: float = 5) -> None:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if condition():
            return
        time.sleep(0.01)
    raise AssertionError("the condition never held")


def dispatch_body(*work: dict, node: dict | None = None) -> dict:
    return {"node": node or NODE, "work": list(work)}


def work(number: int, kind: str = "run_script", **fields) -> dict:
    body = {
        "pid": pid(number),
        "campaign_id": None,
        "attempt": None,
        "kind": kind,
        "script": "echo hello",
        "timeout_seconds": 30,
    }
    body.update(fields)
    return body


def finished(dawn: Dawn) -> list[tuple[str, str, str]]:
    return [
        (message["pid"], message["action_kind"], message["status"])
        for message in dawn.producer.results()
        if message["status"] != "started"
    ]


def test_a_dispatch_is_accepted_and_runs(dawn: Dawn, client: TestClient):
    dawn.fleet.add(FakeNode())

    response = client.post(
        "/v1/dispatch",
        json=dispatch_body(work(1), work(2, "quarantine")),
        headers=bearer(DISPATCHER_TOKEN),
    )

    assert response.status_code == 202, response.text
    assert response.json() == {"accepted": [pid(1), pid(2)]}
    eventually(lambda: len(finished(dawn)) == 2)
    assert finished(dawn) == [
        (pid(2), "quarantine", "succeeded"),
        (pid(1), "run_script", "succeeded"),
    ]


def test_a_dispatch_to_a_nightfall_dawn_does_not_allow_is_malformed(dawn, client):
    response = client.post(
        "/v1/dispatch",
        json=dispatch_body(
            work(1), node={**NODE, "nightfall": "attacker.example.org:8444"}
        ),
        headers=bearer(DISPATCHER_TOKEN),
    )

    assert response.status_code == 400
    assert "allowed_inner_addresses" in response.json()["error"]


def test_a_dispatch_to_an_allowed_nightfall_goes_there(dawn, client):
    dawn.fleet.add(FakeNode())

    response = client.post(
        "/v1/dispatch",
        json=dispatch_body(
            work(1),
            node={**NODE, "nightfall": "nightfall-1.nightfall-inner.dusk.svc:8444"},
        ),
        headers=bearer(DISPATCHER_TOKEN),
    )

    assert response.status_code == 202
    eventually(lambda: finished(dawn))
    assert dawn.fleet.targets[0].host == "nightfall-1.nightfall-inner.dusk.svc"


def test_a_busy_node_answers_too_many_requests(tmp_path):
    dawn = Dawn(tmp_path, limits={"max_work_per_node": 1})
    node = dawn.fleet.add(FakeNode())
    node.scripts["slow"] = [Wait(0.5)]
    with TestClient(dawn.application, base_url="https://dawn") as client:
        first = client.post(
            "/v1/dispatch",
            json=dispatch_body(work(1, script="slow")),
            headers=bearer(DISPATCHER_TOKEN),
        )
        eventually(lambda: "slow" in node.commands_in(int(pid(1))))
        second = client.post(
            "/v1/dispatch",
            json=dispatch_body(work(2)),
            headers=bearer(DISPATCHER_TOKEN),
        )
        third = client.post(
            "/v1/dispatch",
            json=dispatch_body(work(3)),
            headers=bearer(DISPATCHER_TOKEN),
        )
        eventually(lambda: len(finished(dawn)) == 2)

    assert (first.status_code, second.status_code, third.status_code) == (202, 202, 429)


@pytest.mark.parametrize(
    ("fields", "named"),
    [
        ({"pid": "0x1234"}, "pid"),
        ({"pid": "0"}, "pid"),
        ({"pid": "65535"}, "reserved"),
        ({"pid": str(DEFAULT_SH_PID)}, "reserved"),
        ({"pid": str(2**64)}, "64 bits"),
        ({"attempt": 0}, "attempt"),
        ({"script": None}, "needs script"),
        ({"timeout_seconds": None}, "timeout_seconds"),
    ],
)
def test_a_malformed_dispatch_is_a_bad_request(dawn, client, fields, named):
    response = client.post(
        "/v1/dispatch",
        json=dispatch_body(work(1, **fields)),
        headers=bearer(DISPATCHER_TOKEN),
    )

    assert response.status_code == 400
    assert named in response.json()["error"]


def test_a_reap_is_accepted_and_reaps_each_pid(dawn, client):
    node = dawn.fleet.add(FakeNode())
    node.processes[int(pid(7))] = ["sh[server]", "RR"]

    response = client.post(
        "/v1/reap",
        json={"node": NODE, "pids": [pid(7), pid(8)]},
        headers=bearer(DISPATCHER_TOKEN),
    )

    assert response.status_code == 202, response.text
    assert response.json() == {"accepted": [pid(7), pid(8)]}
    eventually(lambda: len(finished(dawn)) == 2)
    assert int(pid(7)) not in node.processes
    assert finished(dawn) == [
        (pid(7), "reap", "reaped"),
        (pid(8), "reap", "reaped"),
    ]


def test_a_reap_needs_a_pid_and_a_dispatcher(dawn, client):
    empty = client.post(
        "/v1/reap", json={"node": NODE, "pids": []}, headers=bearer(DISPATCHER_TOKEN)
    )
    operator = client.post(
        "/v1/reap",
        json={"node": NODE, "pids": [pid(7)]},
        headers=bearer(OPERATOR_TOKEN),
    )

    assert empty.status_code == 400
    assert operator.status_code == 403


def facts_body(**fields) -> dict:
    body = {"node": NODE, "pid": str(PID)}
    body.update(fields)
    return body


def test_facts_answer_with_the_nodes_facts_and_report_them(dawn, client):
    node = dawn.fleet.add(FakeNode())

    response = client.post(
        "/v1/facts", json=facts_body(), headers=bearer(DISPATCHER_TOKEN)
    )

    assert response.status_code == 200, response.text
    body = response.json()
    assert "dusk.device.id" not in body["facts"]
    assert body["facts"]["dusk.hostname"] == "node-1"
    assert body["reported"]["services"] == ["nightfall", "sh[server]"]
    assert finished(dawn) == [(str(PID), "collect_facts", "succeeded")]
    assert dawn.producer.results()[0]["delivered"] is True
    assert node.commands_in(PID)[0] == "kvs get dusk.; ps"
    assert PID not in node.processes
    assert dawn.sessions.open == 0


@pytest.mark.parametrize(
    ("prepare", "status", "result"),
    [
        (lambda fleet: None, 502, "unreachable"),
        (
            lambda fleet: setattr(
                fleet.add(FakeNode()),
                "refusal",
                RuntimeError(
                    "Unimplemented: remote exception: not permitted: Dusk.process"
                ),
            ),
            403,
            "denied",
        ),
    ],
)
def test_facts_from_a_node_that_cannot_answer_say_why(
    dawn, client, prepare, status, result
):
    prepare(dawn.fleet)

    response = client.post(
        "/v1/facts", json=facts_body(), headers=bearer(DISPATCHER_TOKEN)
    )

    assert response.status_code == status
    assert finished(dawn) == [(str(PID), "collect_facts", result)]
    assert dawn.sessions.open == 0


def test_facts_the_node_does_not_read_in_time_are_504(tmp_path):
    dawn = Dawn(tmp_path, limits={"process_timeout_default": 1})
    node = dawn.fleet.add(FakeNode())
    node.commands["ps"] = [Wait(5)]

    with TestClient(dawn.application, base_url="https://dawn-0.dawn:8443") as test:
        response = test.post(
            "/v1/facts", json=facts_body(), headers=bearer(DISPATCHER_TOKEN)
        )

    assert response.status_code == 504, response.text
    assert finished(dawn) == [(str(PID), "collect_facts", "timed_out")]
    assert dawn.sessions.open == 0
    assert node.connections[0].disconnected


def logs_body(**fields) -> dict:
    body = {
        "node": NODE,
        "pid": str(PID),
        "level": "info",
        "duration_seconds": 60,
        "endpoint": None,
    }
    body.update(fields)
    return body


def test_a_log_stream_starts_lists_and_stops(dawn, client):
    node = dawn.fleet.add(FakeNode())
    node.commands["logs"] = [Wait(60)]

    started = client.post(
        "/v1/logs", json=logs_body(), headers=bearer(DISPATCHER_TOKEN)
    )
    stream_id = started.json()["stream_id"]
    listed = client.get("/v1/logs", headers=bearer(VIEWER_TOKEN))
    eventually(lambda: node.commands_in(PID))
    stopped = client.delete(f"/v1/logs/{stream_id}", headers=bearer(DISPATCHER_TOKEN))
    after = client.get("/v1/logs", headers=bearer(VIEWER_TOKEN))
    again = client.delete(f"/v1/logs/{stream_id}", headers=bearer(DISPATCHER_TOKEN))

    assert started.status_code == 202
    assert [stream["stream_id"] for stream in listed.json()] == [stream_id]
    assert listed.json()[0]["principal"] == "twilight-0"
    assert listed.json()[0]["pid"] == str(PID)
    assert stopped.status_code == 204
    assert after.json() == []
    assert again.status_code == 404
    assert finished(dawn) == [(str(PID), "stream_logs", "succeeded")]


def test_a_log_stream_to_an_endpoint_dawn_does_not_allow_is_refused(dawn, client):
    response = client.post(
        "/v1/logs",
        json=logs_body(endpoint="exfiltrate.example.org:4317"),
        headers=bearer(DISPATCHER_TOKEN),
    )

    assert response.status_code == 400
    assert "collector.allowed_endpoints" in response.json()["error"]


def test_a_log_stream_longer_than_allowed_is_refused(dawn, client):
    response = client.post(
        "/v1/logs",
        json=logs_body(duration_seconds=86401),
        headers=bearer(DISPATCHER_TOKEN),
    )

    assert response.status_code == 400


def test_a_log_stream_is_stopped_only_by_whoever_started_it_or_a_dispatcher(
    dawn, client
):
    node = dawn.fleet.add(FakeNode())
    node.commands["logs"] = [Wait(60)]
    started = client.post("/v1/logs", json=logs_body(), headers=bearer(OPERATOR_TOKEN))
    stream_id = started.json()["stream_id"]

    other = client.delete(
        f"/v1/logs/{stream_id}", headers=bearer(SECOND_OPERATOR_TOKEN)
    )
    dispatcher = client.delete(
        f"/v1/logs/{stream_id}", headers=bearer(DISPATCHER_TOKEN)
    )

    assert other.status_code == 404
    assert dispatcher.status_code == 204


def files_body(**fields) -> dict:
    body = {
        "node": NODE,
        "pid": str(PID),
        "path": "/var/log/app.log",
        "campaign_id": None,
    }
    body.update(fields)
    return body


def test_a_file_collection_is_accepted_and_stored(dawn, client):
    node = dawn.fleet.add(FakeNode())
    node.files["/var/log/app.log"] = [b"hello"]

    response = client.post(
        "/v1/files", json=files_body(), headers=bearer(DISPATCHER_TOKEN)
    )

    assert response.status_code == 202
    assert len(response.json()["upload_id"]) == 36
    eventually(lambda: finished(dawn))
    assert dawn.storage.objects == {
        f"files/{DEVICE_ID}/{INSTALLATION_ID}/{PID}/0-app.log": b"hello"
    }
    assert dawn.producer.on("dusk.files")[0]["size_bytes"] == 5


def connect_body(**fields) -> dict:
    body = {"node": NODE, "pid": str(PID)}
    body.update(fields)
    return body


def test_an_operator_opens_an_interactive_session_in_the_shell_at_its_pid(dawn, client):
    node = dawn.fleet.add(FakeNode())
    node.commands["hostname"] = ["node-1"]

    opened = client.post(
        "/v1/connect", json=connect_body(), headers=bearer(OPERATOR_TOKEN)
    )
    descriptor = opened.json()["descriptor"]
    ran = client.post(
        "/v1/sh",
        json={"descriptor": descriptor, "command": "hostname"},
        headers=bearer(OPERATOR_TOKEN),
    )
    closed = client.post(
        "/v1/disconnect",
        json={"descriptor": descriptor},
        headers=bearer(OPERATOR_TOKEN),
    )

    assert opened.status_code == 200, opened.text
    assert ran.json() == {"output": ["node-1"]}
    assert closed.status_code == 200
    assert node.calls[0] == ("connect", PID)
    assert node.commands_in(PID) == ["hostname"]
    eventually(lambda: PID not in node.processes)
    (result,) = dawn.producer.results()
    assert (result["action_kind"], result["status"], result["delivered"]) == (
        "interactive",
        "succeeded",
        True,
    )
    assert all(connection.disconnected for connection in node.connections)
    assert dawn.sessions.open == 0


def test_a_session_that_does_not_come_up_in_time_is_a_bad_gateway(tmp_path):
    impatient = Dawn(tmp_path, nightfall={"connect_timeout_seconds": 0.1})
    impatient.fleet.add(FakeNode())
    impatient.fleet.delay = 0.5

    with TestClient(impatient.application, base_url="https://dawn-0.dawn:8443") as test:
        response = test.post(
            "/v1/connect", json=connect_body(), headers=bearer(OPERATOR_TOKEN)
        )

    assert response.status_code == 502, response.text
    assert "within 0.1 s" in response.json()["error"]
    assert finished(impatient) == [(str(PID), "interactive", "unreachable")]
    assert impatient.sessions.open == 0


def test_another_operator_cannot_use_a_session(dawn, client):
    dawn.fleet.add(FakeNode())
    descriptor = client.post(
        "/v1/connect", json=connect_body(), headers=bearer(OPERATOR_TOKEN)
    ).json()["descriptor"]

    stolen = client.post(
        "/v1/sh",
        json={"descriptor": descriptor, "command": "ps"},
        headers=bearer(SECOND_OPERATOR_TOKEN),
    )

    assert stolen.status_code == 404


def test_a_session_on_an_unreachable_node_is_a_bad_gateway(dawn, client):
    response = client.post(
        "/v1/connect", json=connect_body(), headers=bearer(OPERATOR_TOKEN)
    )

    assert response.status_code == 502
    assert finished(dawn) == [(str(PID), "interactive", "unreachable")]
    assert dawn.sessions.open == 0


def test_a_session_nightfall_refuses_is_forbidden(dawn, client):
    dawn.fleet.add(FakeNode()).refusal = RuntimeError(
        "Unimplemented: remote exception: not permitted: Dusk.process"
    )

    response = client.post(
        "/v1/connect", json=connect_body(), headers=bearer(OPERATOR_TOKEN)
    )

    assert response.status_code == 403
    assert "nightfall refused the session" in response.json()["error"]
    assert finished(dawn) == [(str(PID), "interactive", "denied")]


def test_a_dispatcher_cannot_open_interactive_sessions(dawn, client):
    response = client.post(
        "/v1/connect", json=connect_body(), headers=bearer(DISPATCHER_TOKEN)
    )

    assert response.status_code == 403


def test_a_second_session_at_one_pid_is_a_conflict(dawn, client):
    dawn.fleet.add(FakeNode())

    first = client.post(
        "/v1/connect", json=connect_body(), headers=bearer(OPERATOR_TOKEN)
    )
    second = client.post(
        "/v1/connect", json=connect_body(), headers=bearer(OPERATOR_TOKEN)
    )

    assert first.status_code == 200
    assert second.status_code == 409
    assert "already has an open session" in second.json()["error"]
    assert dawn.sessions.open == 1


def test_interactive_sessions_are_bounded_apart_from_node_sessions(tmp_path):
    bounded = Dawn(tmp_path, limits={"max_interactive_sessions": 1})
    bounded.fleet.add(FakeNode())

    with TestClient(bounded.application, base_url="https://dawn-0.dawn:8443") as test:
        first = test.post(
            "/v1/connect", json=connect_body(), headers=bearer(OPERATOR_TOKEN)
        )
        second = test.post(
            "/v1/connect",
            json=connect_body(pid=str(PID + 1)),
            headers=bearer(OPERATOR_TOKEN),
        )

    assert first.status_code == 200
    assert second.status_code == 503
    assert "limits.max_interactive_sessions" in second.json()["error"]


def test_an_interactive_session_closes_at_the_end_of_its_lifetime(tmp_path):
    brief = Dawn(tmp_path, limits={"interactive_session_seconds": 1})
    node = brief.fleet.add(FakeNode())

    with TestClient(brief.application, base_url="https://dawn-0.dawn:8443") as test:
        descriptor = test.post(
            "/v1/connect", json=connect_body(), headers=bearer(OPERATOR_TOKEN)
        ).json()["descriptor"]
        eventually(lambda: PID not in node.processes)
        ran = test.post(
            "/v1/sh",
            json={"descriptor": descriptor, "command": "ps"},
            headers=bearer(OPERATOR_TOKEN),
        )

    assert ran.status_code == 404
    assert finished(brief) == [(str(PID), "interactive", "succeeded")]
    assert brief.sessions.open == 0


def test_programs_dawn_runs_may_not_take_over_its_terminal(dawn, monkeypatch):
    monkeypatch.delenv("DUSK_NON_INTERACTIVE", raising=False)

    Dawn(dawn.settings.auth.tokens_file.parent)

    assert os.environ["DUSK_NON_INTERACTIVE"] == "1"


def test_health_is_open_and_readiness_reports_its_checks(dawn, client):
    alive = client.get("/healthz")
    ready = client.get("/readyz")

    assert alive.status_code == 200
    assert ready.status_code == 200
    assert ready.json() == {"ready": True, "checks": {"kafka": True, "accepting": True}}


def test_readiness_waits_for_kafka(dawn, client):
    dawn.services.kafka_ready = lambda: False

    ready = client.get("/readyz")

    assert ready.status_code == 503
    assert ready.json()["checks"]["kafka"] is False


def test_the_openapi_document_describes_dawns_api(dawn, client):
    document = client.get("/v1/openapi.json", headers=bearer(VIEWER_TOKEN)).json()

    assert document["info"]["title"] == "dawn"
    assert set(document["paths"]) == {
        "/connect",
        "/disconnect",
        "/sh",
        "/sh/stream",
        "/help",
        "/help/{program}",
        "/dispatch",
        "/reap",
        "/facts",
        "/logs",
        "/logs/{stream_id}",
        "/files",
    }
    connect = document["paths"]["/connect"]["post"]["requestBody"]["content"][
        "application/json"
    ]
    assert connect["schema"]["$ref"].endswith("/ConnectRequest")
    assert "202" in document["paths"]["/dispatch"]["post"]["responses"]
    assert "202" in document["paths"]["/reap"]["post"]["responses"]


def test_help_is_readable_by_any_role(dawn, client):
    response = client.get("/v1/help", headers=bearer(VIEWER_TOKEN))

    assert response.status_code == 200
    assert response.json()[0]["name"] == "ps"


def test_requests_are_counted(dawn, client):
    from prometheus_client import generate_latest

    client.get("/v1/help", headers=bearer(VIEWER_TOKEN))

    assert (
        'dawn_requests_total{method="GET",route="/v1/help",status="200"} 1.0'
        in generate_latest(dawn.metrics.registry).decode()
    )


@pytest.mark.parametrize(
    ("route", "body", "token"),
    [
        ("/v1/dispatch", dispatch_body(work(1)), DISPATCHER_TOKEN),
        ("/v1/reap", {"node": NODE, "pids": [pid(1)]}, DISPATCHER_TOKEN),
        ("/v1/facts", facts_body(), DISPATCHER_TOKEN),
        ("/v1/logs", logs_body(), DISPATCHER_TOKEN),
        ("/v1/files", files_body(), DISPATCHER_TOKEN),
        ("/v1/connect", connect_body(), OPERATOR_TOKEN),
    ],
)
def test_a_draining_dawn_takes_no_new_work_on_any_route(
    dawn, client, route, body, token
):
    dawn.fleet.add(FakeNode())
    dawn.services.draining = True

    response = client.post(route, json=body, headers=bearer(token))

    assert response.status_code == 503
    assert dawn.fleet.targets == []
