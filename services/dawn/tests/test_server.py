from __future__ import annotations

import asyncio
import os
import signal
import socket
import ssl
from pathlib import Path

import httpx
import pytest
from conftest import (
    OPERATOR_TOKEN,
    PROGRAMS,
    CertificateAuthority,
    Dawn,
    FakeFleet,
    bearer,
    dawn_settings,
)
from cryptography import x509

from dawn import server
from dawn.__main__ import serve
from dawn.config import TlsSettings
from dawn.metrics import Metrics
from dawn.server import (
    main_server,
    metrics_application,
    reload_certificates,
    server_ssl_context,
)

pytestmark = pytest.mark.anyio


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


class Pki:
    def __init__(self, directory: Path) -> None:
        self.directory = directory
        self.internal = CertificateAuthority()
        self.ca = self.internal.write_root(directory)
        key, certificate = self.internal.issue(ip_addresses=("127.0.0.1",), server=True)
        self.server_certificate, self.server_key = self.internal.write(
            directory, "server", key, certificate
        )
        self.server_serial = certificate.serial_number

    def client(
        self, stem: str, *uris: str, authority: CertificateAuthority | None = None
    ) -> tuple[str, str]:
        key, certificate = (authority or self.internal).issue(uris=uris)
        return (authority or self.internal).write(
            self.directory, stem, key, certificate
        )


@pytest.fixture
def pki(tmp_path: Path) -> Pki:
    return Pki(tmp_path)


async def running(dawn: Dawn, pki: Pki):
    tls = TlsSettings(
        certificate=Path(pki.server_certificate),
        key=Path(pki.server_key),
        client_ca=Path(pki.ca),
    )
    context = server_ssl_context(tls)
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    port = listener.getsockname()[1]
    instance = main_server(dawn.application, "127.0.0.1", port, context, 1)
    task = asyncio.create_task(instance.serve(sockets=[listener]))
    while not instance.started:
        await asyncio.sleep(0.01)
    return instance, task, f"https://127.0.0.1:{port}", context


async def stopped(instance, task) -> None:
    instance.should_exit = True
    await task


def client(pki: Pki, certificate: tuple[str, str] | None = None) -> httpx.AsyncClient:
    verify = ssl.create_default_context(cafile=pki.ca)
    if certificate is not None:
        verify.load_cert_chain(*certificate)
    return httpx.AsyncClient(verify=verify)


@pytest.fixture
def tls_dawn(tmp_path: Path, pki: Pki) -> Dawn:
    return Dawn(tmp_path, tls={"client_ca": pki.ca})


async def test_a_principal_certificate_authenticates_over_mutual_tls(tls_dawn, pki):
    instance, task, base, _ = await running(tls_dawn, pki)
    try:
        async with client(
            pki, pki.client("twilight", "urn:dusk:principal:twilight-0")
        ) as http:
            response = await http.get(f"{base}/v1/logs")
    finally:
        await stopped(instance, task)

    assert response.status_code == 200, response.text
    assert response.json() == []


async def test_without_a_certificate_or_token_nothing_but_health_answers(tls_dawn, pki):
    instance, task, base, _ = await running(tls_dawn, pki)
    try:
        async with client(pki) as http:
            refused = await http.get(f"{base}/v1/logs")
            health = await http.get(f"{base}/healthz")
    finally:
        await stopped(instance, task)

    assert refused.status_code == 401
    assert health.status_code == 200


@pytest.mark.parametrize(
    "headers",
    [
        {
            "x-forwarded-client-cert": "By=spiffe://dusk;URI=urn:dusk:principal:twilight-0"
        },
        {
            "x-client-cert": "urn:dusk:principal:twilight-0",
            "x-forwarded-for": "10.0.0.1",
        },
        {"x-forwarded-user": "twilight-0", "x-forwarded-proto": "https"},
    ],
)
async def test_proxy_headers_do_not_authenticate_over_tls(tls_dawn, pki, headers):
    instance, task, base, _ = await running(tls_dawn, pki)
    try:
        async with client(pki) as http:
            response = await http.get(f"{base}/v1/logs", headers=headers)
    finally:
        await stopped(instance, task)

    assert response.status_code == 401


async def test_a_node_certificate_cannot_call_dawn(tls_dawn, pki):
    instance, task, base, _ = await running(tls_dawn, pki)
    try:
        certificate = pki.client(
            "node", "urn:dusk:device:" + "0" * 32, "urn:dusk:installation:" + "1" * 32
        )
        async with client(pki, certificate) as http:
            response = await http.get(f"{base}/v1/logs")
    finally:
        await stopped(instance, task)

    assert response.status_code == 403


async def test_a_certificate_from_another_authority_fails_the_handshake(tls_dawn, pki):
    stranger = CertificateAuthority("somebody else's CA")
    instance, task, base, _ = await running(tls_dawn, pki)
    try:
        certificate = pki.client(
            "stranger", "urn:dusk:principal:twilight-0", authority=stranger
        )
        async with client(pki, certificate) as http:
            with pytest.raises(httpx.HTTPError):
                await http.get(f"{base}/v1/logs")
    finally:
        await stopped(instance, task)


async def test_a_bearer_token_works_without_a_certificate(tls_dawn, pki):
    instance, task, base, _ = await running(tls_dawn, pki)
    try:
        async with client(pki) as http:
            response = await http.get(f"{base}/v1/help", headers=bearer(OPERATOR_TOKEN))
    finally:
        await stopped(instance, task)

    assert response.status_code == 200


async def test_a_body_larger_than_the_limit_is_refused(tls_dawn, pki):
    instance, task, base, _ = await running(tls_dawn, pki)
    try:
        async with client(pki) as http:
            response = await http.post(
                f"{base}/v1/connect",
                content=b"x" * (server.MAX_BODY_BYTES + 1),
                headers=bearer(OPERATOR_TOKEN),
            )
    finally:
        await stopped(instance, task)

    assert response.status_code == 413


async def test_a_rotated_server_certificate_is_served_to_new_connections(
    tls_dawn, pki, monkeypatch
):
    monkeypatch.setattr(server, "RELOAD_SECONDS", 0.05)
    instance, task, base, context = await running(tls_dawn, pki)
    reloading = asyncio.create_task(
        reload_certificates(
            context,
            Path(pki.server_certificate),
            Path(pki.server_key),
            Path(pki.ca),
            "server",
        )
    )
    try:
        await asyncio.sleep(0.1)
        key, renewed = pki.internal.issue(ip_addresses=("127.0.0.1",), server=True)
        pki.internal.write(pki.directory, "server", key, renewed)
        stat = os.stat(pki.server_certificate)
        os.utime(
            pki.server_certificate,
            ns=(stat.st_atime_ns, stat.st_mtime_ns + 1_000_000_000),
        )
        await asyncio.sleep(0.3)
        port = int(base.rsplit(":", 1)[1])
        verify = ssl.create_default_context(cafile=pki.ca)
        reader, writer = await asyncio.open_connection("127.0.0.1", port, ssl=verify)
        served = writer.get_extra_info("ssl_object").getpeercert(binary_form=True)
        writer.close()
    finally:
        reloading.cancel()
        await stopped(instance, task)

    assert x509.load_der_x509_certificate(served).serial_number == renewed.serial_number
    assert renewed.serial_number != pki.server_serial


async def test_the_metrics_listener_exposes_metrics_and_readiness():
    metrics = Metrics()
    metrics.node_sessions.set(4)
    ready = {"kafka": False, "accepting": True}
    application = metrics_application(metrics, lambda: ready)

    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app=application), base_url="http://dawn"
    ) as http:
        exposition = await http.get("/metrics")
        readiness = await http.get("/readyz")
        health = await http.get("/healthz")

    assert "dawn_node_sessions 4.0" in exposition.text
    assert readiness.status_code == 503
    assert readiness.json()["checks"]["kafka"] is False
    assert health.status_code == 200


async def test_dawn_serves_until_sigterm_then_drains_and_stops(
    tmp_path: Path, pki: Pki
):
    (tmp_path / "tokens.toml").write_text("")
    (tmp_path / "output.key").write_bytes(os.urandom(32).hex().encode())
    (tmp_path / "s3-access").write_text("testing")
    (tmp_path / "s3-secret").write_text("testing")
    listen, metrics_listen = free_port(), free_port()
    settings = dawn_settings(
        listen=f"127.0.0.1:{listen}",
        metrics_listen=f"127.0.0.1:{metrics_listen}",
        drain_seconds=5,
        output_key_file=str(tmp_path / "output.key"),
        tls={
            "certificate": pki.server_certificate,
            "key": pki.server_key,
            "client_ca": pki.ca,
        },
        auth={"tokens_file": str(tmp_path / "tokens.toml")},
        kafka={"brokers": "127.0.0.1:1", "allow_plaintext": True},
        s3={
            "endpoint": "http://127.0.0.1:1",
            "access_key_file": str(tmp_path / "s3-access"),
            "secret_key_file": str(tmp_path / "s3-secret"),
        },
    )
    serving = asyncio.create_task(serve(settings, FakeFleet(), PROGRAMS))
    async with httpx.AsyncClient() as plain, client(pki) as secure:
        for _attempt in range(500):
            try:
                health = await plain.get(f"http://127.0.0.1:{metrics_listen}/healthz")
                break
            except httpx.ConnectError:
                await asyncio.sleep(0.02)
        else:
            raise AssertionError("dawn did not answer on its metrics port in 10 s")
        main_health = await secure.get(f"https://127.0.0.1:{listen}/healthz")
        readiness = await plain.get(f"http://127.0.0.1:{metrics_listen}/readyz")
        exposition = await plain.get(f"http://127.0.0.1:{metrics_listen}/metrics")

    os.kill(os.getpid(), signal.SIGTERM)
    await asyncio.wait_for(serving, timeout=20)

    assert health.status_code == 200
    assert main_health.status_code == 200
    assert readiness.status_code == 503
    assert readiness.json()["checks"] == {"kafka": False, "accepting": True}
    assert "dawn_node_sessions" in exposition.text
