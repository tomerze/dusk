#![allow(unused_imports)]

#[cfg(test)]
mod sh_view;

use dusk_tests::{DuskNixImpl, LISTEN_ADDRESS, gen_port};
use std::process::Command;

#[allow(dead_code)]
fn run_python_code(code: &str) -> bool {
    let python = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.venv/bin/python");
    let status = Command::new(python)
        .arg("-c")
        .arg(code)
        .status()
        .expect("failed to run python code");

    status.success()
}

#[test]
fn test_sanity() {
    let port = gen_port();
    let address = LISTEN_ADDRESS;
    let _dusk = DuskNixImpl::new(address, port);

    let code = format!(
        r#"
import dusk
client = dusk.Dusk("{address}", {port})
client.disconnect()
"#,
        address = address,
        port = port
    );

    assert!(run_python_code(&code));
}

/// Drives the REST API of a real `dusk_gw` process against a real node.
///
/// The gateway's own tests (`tests/gw`) stub the node out to stay fast, so this
/// is the one place the whole path is exercised: an HTTP request arrives on a
/// bound socket, the gateway opens a Cap'n Proto connection to a node, runs a
/// program, and hands the output back as JSON.
#[test]
fn test_gateway_rest_api_over_http() {
    let port = gen_port();
    let address = LISTEN_ADDRESS;
    let _dusk = DuskNixImpl::new(address, port);

    let mut gateway_port = gen_port();
    while gateway_port == port {
        gateway_port = gen_port();
    }

    let code = format!(
        r#"
import re
import socket
import subprocess
import sys
import time

import httpx

gateway = subprocess.Popen([sys.executable, "-m", "dusk.gw", "{address}", "{gateway_port}"])
base = "http://{address}:{gateway_port}"
try:
    for _ in range(120):
        if gateway.poll() is not None:
            raise AssertionError(f"gateway exited with {{gateway.returncode}}")
        try:
            httpx.get(base + "/v1/help", timeout=1.0)
            break
        except httpx.HTTPError:
            time.sleep(0.5)
    else:
        raise AssertionError("gateway never started answering")

    programs = httpx.get(base + "/v1/help").json()
    assert any(program["name"] == "hostname" for program in programs), programs
    assert httpx.get(base + "/v1/help/hostname").json()["name"] == "hostname"
    assert httpx.get(base + "/v1/help/rm").status_code == 404

    specification = httpx.get(base + "/v1/openapi.json")
    assert specification.status_code == 200, specification.text
    assert sorted(specification.json()["paths"]) == [
        "/connect", "/disconnect", "/help", "/help/{{program}}", "/sh", "/sh/stream",
    ], sorted(specification.json()["paths"])

    swagger = httpx.get(base + "/v1/docs")
    assert swagger.status_code == 200, swagger.text
    assert "/v1/openapi.json" in swagger.text
    # Nothing on the docs page may come from anywhere but the gateway, and the
    # assets it does ask for have to be there: this is the check that a wheel
    # missing its vendored Swagger UI fails.
    assert not re.findall(r'''["'(](https?://[^"')\s]+)''', swagger.text), swagger.text
    for asset in ("swagger-ui-bundle.js", "swagger-ui.css"):
        served = httpx.get(base + "/v1/static/" + asset)
        assert served.status_code == 200, asset
        assert len(served.content) > 10000, (asset, len(served.content))

    opened = httpx.post(base + "/v1/connect", json={{"host": "{address}", "port": {port}}})
    assert opened.status_code == 200, opened.text
    descriptor = opened.json()["descriptor"]

    ran = httpx.post(base + "/v1/sh", json={{"descriptor": descriptor, "command": "hostname"}})
    assert ran.status_code == 200, ran.text
    assert ran.json()["output"], ran.text

    # The same command over SSE, against the real node: the events have to
    # arrive and the stream has to end on its own.
    streamed = []
    with httpx.stream(
        "POST", base + "/v1/sh/stream", timeout=30,
        json={{"descriptor": descriptor, "command": "hostname"}},
    ) as response:
        assert response.status_code == 200, response.read()
        assert response.headers["content-type"].startswith("text/event-stream")
        for line in response.iter_lines():
            if line.startswith("event: "):
                streamed.append(line.removeprefix("event: "))
                if streamed[-1] in ("end", "error"):
                    break
    assert streamed[0] == "start", streamed
    assert streamed[-1] == "end", streamed
    assert "output" in streamed, streamed

    closed = httpx.post(base + "/v1/disconnect", json={{"descriptor": descriptor}})
    assert closed.status_code == 200, closed.text

    reused = httpx.post(base + "/v1/sh", json={{"descriptor": descriptor, "command": "ps"}})
    assert reused.status_code == 404, reused.text

    # A port the kernel just handed out and we gave straight back, so nothing
    # is listening on it and the gateway's connect attempt is refused.
    with socket.socket() as probe:
        probe.bind(("{address}", 0))
        unused_port = probe.getsockname()[1]
    refused = httpx.post(base + "/v1/connect", json={{"host": "{address}", "port": unused_port}})
    assert refused.status_code == 502, refused.text
finally:
    gateway.terminate()
    gateway.wait(timeout=30)
"#
    );

    assert!(run_python_code(&code));
}

#[test]
fn test_multiple_clients_same_server() {
    let port = gen_port();
    let address = LISTEN_ADDRESS;
    let _dusk = DuskNixImpl::new(address, port);

    let code = format!(
        r#"
import dusk
client = dusk.Dusk("{address}", {port})
another_client = dusk.Dusk("{address}", {port})
client.disconnect()
another_client.disconnect()
"#,
        address = address,
        port = port
    );

    assert!(run_python_code(&code));
}
