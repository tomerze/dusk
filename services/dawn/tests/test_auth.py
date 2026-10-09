from __future__ import annotations

import hashlib
import json
import os
import time
from pathlib import Path

import httpx
import jwt
import pytest
from conftest import CertificateAuthority, der
from cryptography.hazmat.primitives.asymmetric import rsa
from jwt.algorithms import RSAAlgorithm
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse
from starlette.routing import Route
from starlette.testclient import TestClient

from dawn.auth import (
    PEER_CERTIFICATE,
    AuthenticationFailed,
    AuthenticationMiddleware,
    Authenticator,
    OidcVerifier,
    TokenFile,
    certificate_principal,
    required_roles,
)
from dawn.keys import UrlKeys

PRINCIPALS = {"twilight-*": "dispatcher", "operator-console-*": "operator"}
ISSUER = "https://id.example.org/realms/dusk"
AUDIENCE = "dawn"

pytestmark = pytest.mark.anyio


@pytest.fixture(scope="module")
def authority() -> CertificateAuthority:
    return CertificateAuthority()


def client_certificate(authority: CertificateAuthority, *uris: str) -> bytes:
    return der(authority.issue(uris=uris)[1])


def test_a_principal_certificate_maps_to_its_role(authority):
    principal = certificate_principal(
        client_certificate(authority, "urn:dusk:principal:twilight-0"), PRINCIPALS
    )

    assert principal.subject == "twilight-0"
    assert principal.roles == {"dispatcher"}
    assert principal.method == "certificate"
    assert not principal.acts_for_itself()


@pytest.mark.parametrize(
    ("uris", "reason"),
    [
        ((), "names no principal"),
        (
            ("urn:dusk:principal:twilight-0", "urn:dusk:principal:twilight-1"),
            "exactly one",
        ),
        (
            ("urn:dusk:principal:twilight-0", "urn:dusk:device:" + "0" * 32),
            "node certificate",
        ),
        (("urn:dusk:installation:" + "0" * 32,), "node certificate"),
        (("urn:dusk:principal:somebody-else",), "has no role"),
        (("urn:dusk:principal:has spaces",), "exactly one"),
    ],
)
def test_a_certificate_that_does_not_name_one_known_principal_is_refused(
    authority, uris, reason
):
    with pytest.raises(AuthenticationFailed) as raised:
        certificate_principal(client_certificate(authority, *uris), PRINCIPALS)

    assert raised.value.status == 403
    assert reason in raised.value.message


def write_tokens(path: Path, *entries: tuple[str, str, str]) -> None:
    lines = []
    for token, role, subject in entries:
        digest = hashlib.sha256(token.encode()).hexdigest()
        lines.append(
            f'[[token]]\nsha256 = "{digest}"\nrole = "{role}"\nsubject = "{subject}"\n'
        )
    path.write_text("\n".join(lines))


def touch_later(path: Path) -> None:
    stat = path.stat()
    os.utime(path, ns=(stat.st_atime_ns, stat.st_mtime_ns + 1_000_000_000))


def test_a_known_bearer_token_is_its_subject_and_role(tmp_path):
    path = tmp_path / "tokens.toml"
    write_tokens(path, ("s3cret-operator", "operator", "operator@example.org"))

    principal = TokenFile(path).principal("s3cret-operator")

    assert principal is not None
    assert principal.subject == "operator@example.org"
    assert principal.roles == {"operator"}
    assert principal.acts_for_itself()


def test_an_unknown_bearer_token_is_nobody(tmp_path):
    path = tmp_path / "tokens.toml"
    write_tokens(path, ("s3cret-operator", "operator", "operator@example.org"))

    assert TokenFile(path).principal("s3cret-operatoR") is None


def test_the_token_file_is_reread_when_it_changes(tmp_path):
    path = tmp_path / "tokens.toml"
    write_tokens(path, ("first", "viewer", "first"))
    tokens = TokenFile(path)

    write_tokens(path, ("second", "operator", "second"))
    touch_later(path)

    assert tokens.principal("first") is None
    second = tokens.principal("second")
    assert second is not None and second.subject == "second"


def test_a_token_file_that_turns_invalid_keeps_the_tokens_loaded_before(tmp_path):
    path = tmp_path / "tokens.toml"
    write_tokens(path, ("first", "viewer", "first"))
    tokens = TokenFile(path)

    path.write_text('[[token]]\nsha256 = "short"\nrole = "viewer"\nsubject = "x"\n')
    touch_later(path)

    assert tokens.principal("first") is not None


@pytest.mark.parametrize(
    "content",
    [
        '[[token]]\nsha256 = "abc"\nrole = "viewer"\nsubject = "x"\n',
        f'[[token]]\nsha256 = "{"a" * 64}"\nrole = "admin"\nsubject = "x"\n',
        f'[[token]]\nsha256 = "{"a" * 64}"\nrole = "viewer"\n',
        f'[[token]]\nsha256 = "{"A" * 64}"\nrole = "viewer"\nsubject = "x"\n',
        "token = 1\n",
        f'[[token]]\nsha256 = "{"a" * 64}"\nrole = "viewer"\nsubject = "x"\n'
        f'[[token]]\nsha256 = "{"a" * 64}"\nrole = "operator"\nsubject = "y"\n',
    ],
)
def test_an_invalid_token_file_is_refused_at_startup(tmp_path, content):
    path = tmp_path / "tokens.toml"
    path.write_text(content)

    with pytest.raises(ValueError):
        TokenFile(path)


class Issuer:
    def __init__(self, key_id: str = "issuer-1") -> None:
        self.key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
        self.key_id = key_id
        entry = json.loads(RSAAlgorithm.to_jwk(self.key.public_key()))
        entry.update({"kid": key_id, "use": "sig", "alg": "RS256"})
        self.key_set = {"keys": [entry]}

    def token(self, headers: dict | None = None, **claims) -> str:
        now = int(time.time())
        payload = {
            "iss": ISSUER,
            "aud": AUDIENCE,
            "sub": "user-7",
            "iat": now,
            "exp": now + 300,
            "groups": ["dusk-operators"],
        }
        payload.update(claims)
        header = {"kid": self.key_id}
        header.update(headers or {})
        return jwt.encode(payload, self.key, algorithm="RS256", headers=header)


@pytest.fixture
def issuer() -> Issuer:
    return Issuer()


async def located() -> str:
    return f"{ISSUER}/protocol/openid-connect/certs"


def oidc(issuer: Issuer) -> OidcVerifier:
    client = httpx.AsyncClient(
        transport=httpx.MockTransport(
            lambda request: httpx.Response(200, json=issuer.key_set)
        )
    )
    return OidcVerifier(
        ISSUER,
        AUDIENCE,
        "groups",
        {"dusk-operators": "operator", "dusk-viewers": "viewer"},
        UrlKeys(located, client, 300),
    )


async def test_an_oidc_token_maps_its_groups_to_roles(issuer):
    principal = await oidc(issuer).principal(
        issuer.token(groups=["dusk-viewers", "dusk-operators", "unmapped"])
    )

    assert principal.subject == "user-7"
    assert principal.roles == {"operator", "viewer"}
    assert principal.method == "oidc"


async def test_a_single_string_role_claim_is_read(issuer):
    principal = await oidc(issuer).principal(issuer.token(groups="dusk-viewers"))

    assert principal.roles == {"viewer"}


@pytest.mark.parametrize(
    ("claims", "status"),
    [
        ({"aud": "someone-else"}, 401),
        ({"iss": "https://id.example.org/realms/other"}, 401),
        ({"exp": int(time.time()) - 3600}, 401),
        ({"groups": ["unmapped"]}, 403),
        ({"groups": None}, 403),
    ],
)
async def test_an_oidc_token_for_somebody_else_or_no_role_is_refused(
    issuer, claims, status
):
    with pytest.raises(AuthenticationFailed) as raised:
        await oidc(issuer).principal(issuer.token(**claims))

    assert raised.value.status == status


async def test_an_oidc_token_from_an_unknown_key_is_refused(issuer):
    other = Issuer(key_id="issuer-9")

    with pytest.raises(AuthenticationFailed) as raised:
        await oidc(issuer).principal(other.token())

    assert raised.value.status == 401


async def test_an_oidc_token_signed_with_a_shared_secret_is_refused(issuer):
    token = jwt.encode(
        {"iss": ISSUER, "aud": AUDIENCE, "sub": "x", "iat": 0, "exp": 2**31},
        "secret-long-enough-for-hs256-to-accept-it",
        algorithm="HS256",
        headers={"kid": "issuer-1"},
    )

    with pytest.raises(AuthenticationFailed) as raised:
        await oidc(issuer).principal(token)

    assert "HS256" in raised.value.message


@pytest.mark.parametrize(
    ("method", "path", "roles"),
    [
        ("GET", "/healthz", None),
        ("GET", "/readyz", None),
        ("POST", "/v1/dispatch", {"dispatcher"}),
        ("POST", "/v1/facts", {"dispatcher"}),
        ("POST", "/v1/logs", {"dispatcher", "operator"}),
        ("DELETE", "/v1/logs/abc", {"dispatcher", "operator"}),
        ("GET", "/v1/logs", {"dispatcher", "operator", "viewer"}),
        ("POST", "/v1/files", {"dispatcher", "operator"}),
        ("POST", "/v1/connect", {"operator"}),
        ("POST", "/v1/sh/stream", {"operator"}),
        ("POST", "/mcp", {"operator"}),
        ("GET", "/v1/help", {"dispatcher", "operator", "viewer"}),
        ("GET", "/v1/help/ps", {"dispatcher", "operator", "viewer"}),
        ("GET", "/v1/openapi.json", {"dispatcher", "operator", "viewer"}),
        ("GET", "/v1/docs", {"dispatcher", "operator", "viewer"}),
        ("GET", "/v1/static/swagger-ui.css", {"dispatcher", "operator", "viewer"}),
        ("POST", "/v1/help", {"operator"}),
        ("GET", "/v1/a-route-dawn-does-not-name", {"operator"}),
        ("GET", "/healthz/", {"operator"}),
    ],
)
def test_each_route_requires_its_roles(method, path, roles):
    assert required_roles(method, path) == roles


def echo(request: Request) -> JSONResponse:
    principal = request.state.principal
    return JSONResponse(
        {"subject": principal.subject, "roles": sorted(principal.roles)}
    )


def health(request: Request) -> JSONResponse:
    return JSONResponse({"state": "ok"})


SESSIONS = iter(range(1000))


def mcp(request: Request) -> JSONResponse:
    if "mcp-session-id" in request.headers:
        return JSONResponse({"session": request.headers["mcp-session-id"]})
    return JSONResponse({}, headers={"mcp-session-id": f"session-{next(SESSIONS)}"})


class PeerCertificate:
    def __init__(self, application, certificate: bytes | None) -> None:
        self.application = application
        self.certificate = certificate

    async def __call__(self, scope, receive, send):
        if self.certificate is not None:
            scope.setdefault("state", {})[PEER_CERTIFICATE] = self.certificate
        await self.application(scope, receive, send)


def guarded(
    tmp_path: Path, issuer: Issuer, certificate: bytes | None = None
) -> TestClient:
    tokens = tmp_path / "tokens.toml"
    write_tokens(
        tokens,
        ("operator-token", "operator", "operator@example.org"),
        ("viewer-token", "viewer", "viewer@example.org"),
        ("second-operator-token", "operator", "second@example.org"),
    )
    application = Starlette(
        routes=[
            Route("/healthz", health),
            Route("/v1/{rest:path}", echo, methods=["GET", "POST", "DELETE"]),
            Route("/mcp", mcp, methods=["GET", "POST", "DELETE"]),
        ]
    )
    authenticator = Authenticator(TokenFile(tokens), oidc(issuer), PRINCIPALS)
    return TestClient(
        PeerCertificate(
            AuthenticationMiddleware(application, authenticator), certificate
        )
    )


def bearer(token: str) -> dict[str, str]:
    return {"authorization": f"Bearer {token}"}


def test_health_needs_no_credentials(tmp_path, issuer):
    assert guarded(tmp_path, issuer).get("/healthz").status_code == 200


def test_a_request_without_credentials_is_refused(tmp_path, issuer):
    response = guarded(tmp_path, issuer).get("/v1/help")

    assert response.status_code == 401
    assert response.headers["www-authenticate"] == "Bearer"
    assert "error" in response.json()


@pytest.mark.parametrize(
    "headers",
    [
        {"x-forwarded-client-cert": "URI=urn:dusk:principal:twilight-0"},
        {"x-ssl-client-cert": "-----BEGIN CERTIFICATE-----"},
        {"x-forwarded-user": "twilight-0"},
        {"x-remote-user": "operator@example.org"},
        {"x-auth-request-user": "operator@example.org"},
        {"forwarded": "for=10.0.0.1;by=twilight"},
    ],
)
def test_proxy_headers_never_authenticate(tmp_path, issuer, headers):
    response = guarded(tmp_path, issuer).post("/v1/dispatch", headers=headers)

    assert response.status_code == 401


def test_proxy_headers_do_not_change_who_a_bearer_is(tmp_path, issuer):
    response = guarded(tmp_path, issuer).get(
        "/v1/help",
        headers={**bearer("viewer-token"), "x-forwarded-user": "operator@example.org"},
    )

    assert response.json()["subject"] == "viewer@example.org"


def test_a_client_certificate_authenticates_its_principal(tmp_path, issuer, authority):
    certificate = client_certificate(authority, "urn:dusk:principal:twilight-3")

    response = guarded(tmp_path, issuer, certificate).post("/v1/dispatch")

    assert response.status_code == 200
    assert response.json() == {"subject": "twilight-3", "roles": ["dispatcher"]}


def test_a_dispatcher_cannot_open_interactive_sessions(tmp_path, issuer, authority):
    certificate = client_certificate(authority, "urn:dusk:principal:twilight-3")

    response = guarded(tmp_path, issuer, certificate).post("/v1/connect")

    assert response.status_code == 403


def test_a_viewer_can_read_but_not_act(tmp_path, issuer):
    client = guarded(tmp_path, issuer)

    assert client.get("/v1/logs", headers=bearer("viewer-token")).status_code == 200
    assert client.post("/v1/logs", headers=bearer("viewer-token")).status_code == 403
    assert client.post("/v1/files", headers=bearer("viewer-token")).status_code == 403
    assert client.post("/v1/sh", headers=bearer("viewer-token")).status_code == 403


def test_an_operator_cannot_dispatch(tmp_path, issuer):
    response = guarded(tmp_path, issuer).post(
        "/v1/dispatch", headers=bearer("operator-token")
    )

    assert response.status_code == 403


def test_an_oidc_bearer_is_accepted(tmp_path, issuer):
    response = guarded(tmp_path, issuer).post(
        "/v1/connect", headers=bearer(issuer.token())
    )

    assert response.status_code == 200
    assert response.json() == {"subject": "user-7", "roles": ["operator"]}


def test_an_unknown_bearer_is_refused_even_with_a_certificate(
    tmp_path, issuer, authority
):
    certificate = client_certificate(authority, "urn:dusk:principal:twilight-3")

    response = guarded(tmp_path, issuer, certificate).post(
        "/v1/dispatch", headers=bearer("guess")
    )

    assert response.status_code == 401


@pytest.mark.parametrize("value", ["Basic b3BlcmF0b3I6eA==", "Bearer", "Bearer   "])
def test_a_malformed_authorization_header_is_refused(tmp_path, issuer, value):
    response = guarded(tmp_path, issuer).get(
        "/v1/help", headers={"authorization": value}
    )

    assert response.status_code == 401


def test_an_mcp_session_belongs_to_the_principal_that_opened_it(tmp_path, issuer):
    client = guarded(tmp_path, issuer)
    opened = client.post("/mcp", headers=bearer("operator-token"))
    session = opened.headers["mcp-session-id"]

    own = client.post(
        "/mcp", headers={**bearer("operator-token"), "mcp-session-id": session}
    )
    stolen = client.post(
        "/mcp", headers={**bearer("second-operator-token"), "mcp-session-id": session}
    )
    invented = client.post(
        "/mcp", headers={**bearer("operator-token"), "mcp-session-id": "session-x"}
    )

    assert own.status_code == 200
    assert own.json() == {"session": session}
    assert stolen.status_code == 404
    assert invented.status_code == 404


def test_an_mcp_session_is_not_shared_by_principals_of_one_subject_name(
    tmp_path, issuer
):
    client = guarded(tmp_path, issuer)
    session = client.post("/mcp", headers=bearer("operator-token")).headers[
        "mcp-session-id"
    ]
    same_name = issuer.token(sub="operator@example.org")

    borrowed = client.post(
        "/mcp", headers={**bearer(same_name), "mcp-session-id": session}
    )

    assert borrowed.status_code == 404


def test_a_deleted_mcp_session_is_forgotten(tmp_path, issuer):
    client = guarded(tmp_path, issuer)
    session = client.post("/mcp", headers=bearer("operator-token")).headers[
        "mcp-session-id"
    ]

    deleted = client.delete(
        "/mcp", headers={**bearer("operator-token"), "mcp-session-id": session}
    )
    reused = client.post(
        "/mcp", headers={**bearer("operator-token"), "mcp-session-id": session}
    )

    assert deleted.status_code == 200
    assert reused.status_code == 404


def test_a_broken_token_file_is_reported_once_until_it_changes(
    tmp_path, caplog: pytest.LogCaptureFixture
):
    path = tmp_path / "tokens.toml"
    write_tokens(path, ("operator-token", "operator", "operator@example.org"))
    tokens = TokenFile(path)
    path.write_text("[[token]\n")
    stat = path.stat()
    os.utime(path, ns=(stat.st_atime_ns, stat.st_mtime_ns + 1_000_000_000))

    for _ in range(3):
        assert tokens.principal("operator-token") is not None

    kept = "kept the bearer tokens loaded before: the file cannot be read"
    assert caplog.messages.count(kept) == 1
