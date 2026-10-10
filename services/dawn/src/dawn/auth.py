from __future__ import annotations

import hashlib
import json
import logging
import re
import time
import tomllib
from collections import OrderedDict
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, get_args

import jwt
from cryptography import x509
from starlette.types import ASGIApp, Message, Receive, Scope, Send

from .config import Role
from .keys import UnknownKey, UrlKeys

logger = logging.getLogger(__name__)

ROLES: frozenset[str] = frozenset(get_args(Role))
DISPATCHER = "dispatcher"
OPERATOR = "operator"
VIEWER = "viewer"
EVERYONE: frozenset[str] = frozenset({DISPATCHER, OPERATOR, VIEWER})
PRINCIPAL_URI = re.compile(r"^urn:dusk:principal:([A-Za-z0-9._-]{1,253})$")
NODE_URI_PREFIXES = ("urn:dusk:device:", "urn:dusk:installation:")
TOKEN_DIGEST = re.compile(r"^[0-9a-f]{64}$")
OIDC_ALGORITHMS = frozenset(
    {"RS256", "RS384", "RS512", "PS256", "PS384", "PS512", "ES256", "ES384", "EdDSA"}
)
OIDC_LEEWAY_SECONDS = 60
MAX_TOKEN_BYTES = 16384
MAX_MCP_SESSIONS = 65536
PEER_CERTIFICATE = "peer_certificate"
PRINCIPAL = "principal"


@dataclass(frozen=True)
class Principal:
    subject: str
    roles: frozenset[str]
    method: Literal["certificate", "token", "oidc"]

    def acts_for_itself(self) -> bool:
        return DISPATCHER not in self.roles


class AuthenticationFailed(Exception):
    def __init__(self, status: int, message: str) -> None:
        super().__init__(message)
        self.status = status
        self.message = message


def matches(pattern: str, text: str) -> bool:
    expression = ".*".join(re.escape(piece) for piece in pattern.split("*"))
    return re.fullmatch(expression, text) is not None


def certificate_principal(der: bytes, principals: Mapping[str, str]) -> Principal:
    try:
        certificate = x509.load_der_x509_certificate(der)
        names = certificate.extensions.get_extension_for_class(
            x509.SubjectAlternativeName
        ).value
    except (ValueError, x509.ExtensionNotFound) as failure:
        raise AuthenticationFailed(
            403, "the client certificate names no principal"
        ) from failure
    uris = names.get_values_for_type(x509.UniformResourceIdentifier)
    if any(uri.startswith(NODE_URI_PREFIXES) for uri in uris):
        raise AuthenticationFailed(403, "a node certificate cannot call dawn")
    found = [match.group(1) for uri in uris if (match := PRINCIPAL_URI.match(uri))]
    if len(found) != 1:
        raise AuthenticationFailed(
            403, "the client certificate must name exactly one urn:dusk:principal"
        )
    subject = found[0]
    roles = frozenset(
        role for pattern, role in principals.items() if matches(pattern, subject)
    )
    if not roles:
        raise AuthenticationFailed(403, f"principal {subject} has no role in dawn")
    return Principal(subject, roles, "certificate")


class TokenFile:
    def __init__(self, path: Path) -> None:
        self._path = path
        self._modified: int | None = None
        self._rejected: int | None = None
        self._failure: str | None = None
        self._tokens: dict[str, Principal] = {}
        self._refresh()

    def _refresh(self) -> None:
        modified = self._path.stat().st_mtime_ns
        if modified in (self._modified, self._rejected):
            return
        try:
            self._load(modified)
        except ValueError, tomllib.TOMLDecodeError:
            self._rejected = modified
            raise

    def _load(self, modified: int) -> None:
        with self._path.open("rb") as file:
            document = tomllib.load(file)
        tokens: dict[str, Principal] = {}
        entries = document.get("token", [])
        if not isinstance(entries, list):
            raise ValueError(f"{self._path}: token is not an array of tables")
        for position, entry in enumerate(entries):
            digest = entry.get("sha256") if isinstance(entry, dict) else None
            role = entry.get("role") if isinstance(entry, dict) else None
            subject = entry.get("subject") if isinstance(entry, dict) else None
            if not isinstance(digest, str) or not TOKEN_DIGEST.match(digest):
                raise ValueError(
                    f"{self._path}: token {position} has no sha256 of 64 lowercase hex digits"
                )
            if role not in ROLES:
                raise ValueError(
                    f"{self._path}: token {position} has role {role!r}, not one of {sorted(ROLES)}"
                )
            if not isinstance(subject, str) or not subject:
                raise ValueError(f"{self._path}: token {position} has no subject")
            if digest in tokens:
                raise ValueError(f"{self._path}: token {position} repeats a sha256")
            tokens[digest] = Principal(subject, frozenset({role}), "token")
        self._tokens = tokens
        self._modified = modified
        logger.info(
            "loaded the bearer tokens",
            extra={"origin": str(self._path), "token_count": len(tokens)},
        )

    def principal(self, token: str) -> Principal | None:
        try:
            self._refresh()
        except (OSError, ValueError, tomllib.TOMLDecodeError) as failure:
            if str(failure) != self._failure:
                self._failure = str(failure)
                logger.warning(
                    "kept the bearer tokens loaded before: the file cannot be read",
                    extra={"origin": str(self._path), "error": str(failure)},
                )
        else:
            self._failure = None
        return self._tokens.get(hashlib.sha256(token.encode()).hexdigest())


class OidcVerifier:
    def __init__(
        self,
        issuer: str,
        audience: str,
        role_claim: str,
        role_map: Mapping[str, str],
        keys: UrlKeys,
        clock: Callable[[], float] = time.time,
    ) -> None:
        self._issuer = issuer
        self._audience = audience
        self._role_claim = role_claim
        self._role_map = dict(role_map)
        self._keys = keys
        self._clock = clock

    async def principal(self, token: str) -> Principal:
        try:
            header = jwt.get_unverified_header(token)
        except jwt.PyJWTError as failure:
            raise AuthenticationFailed(
                401, "the bearer token is not a JWT"
            ) from failure
        algorithm = header.get("alg")
        if algorithm not in OIDC_ALGORITHMS:
            raise AuthenticationFailed(
                401, f"the bearer token is signed with {algorithm!r}"
            )
        key_id = header.get("kid")
        if key_id is not None and not isinstance(key_id, str):
            raise AuthenticationFailed(401, "the bearer token's kid is not a string")
        try:
            key = await self._keys.key(key_id)
        except UnknownKey as failure:
            raise AuthenticationFailed(
                401, "the bearer token is signed by an unknown key"
            ) from failure
        try:
            claims = jwt.decode(
                token,
                key=key,
                algorithms=[algorithm],
                audience=self._audience,
                issuer=self._issuer,
                leeway=OIDC_LEEWAY_SECONDS,
                options={"require": ["exp", "iat", "sub", "iss", "aud"]},
            )
        except jwt.PyJWTError as failure:
            raise AuthenticationFailed(
                401, f"the bearer token does not verify: {failure}"
            ) from failure
        subject = claims["sub"]
        if not isinstance(subject, str) or not subject:
            raise AuthenticationFailed(401, "the bearer token's sub is not a string")
        granted = claims.get(self._role_claim)
        values = [granted] if isinstance(granted, str) else granted
        if not isinstance(values, list):
            values = []
        roles = frozenset(
            self._role_map[value]
            for value in values
            if isinstance(value, str) and value in self._role_map
        )
        if not roles:
            raise AuthenticationFailed(
                403,
                f"{subject} has no {self._role_claim} value that maps to a dawn role",
            )
        return Principal(subject, roles, "oidc")


class Authenticator:
    def __init__(
        self,
        tokens: TokenFile | None,
        oidc: OidcVerifier | None,
        principals: Mapping[str, str],
    ) -> None:
        self._tokens = tokens
        self._oidc = oidc
        self._principals = dict(principals)

    async def authenticate(self, scope: Scope) -> Principal:
        authorization = None
        for name, value in scope.get("headers", []):
            if name == b"authorization":
                authorization = value
                break
        if authorization is not None:
            return await self._bearer(authorization)
        certificate = scope.get("state", {}).get(PEER_CERTIFICATE)
        if certificate is not None:
            return certificate_principal(certificate, self._principals)
        raise AuthenticationFailed(
            401, "authenticate with a bearer token or a client certificate"
        )

    async def _bearer(self, authorization: bytes) -> Principal:
        scheme, _, token_bytes = authorization.partition(b" ")
        if scheme.lower() != b"bearer" or not token_bytes.strip():
            raise AuthenticationFailed(401, "the authorization header is not Bearer")
        if len(token_bytes) > MAX_TOKEN_BYTES:
            raise AuthenticationFailed(401, "the bearer token is too long")
        try:
            token = token_bytes.strip().decode("ascii")
        except UnicodeDecodeError as failure:
            raise AuthenticationFailed(
                401, "the bearer token is not ASCII"
            ) from failure
        if self._oidc is not None and token.count(".") == 2:
            return await self._oidc.principal(token)
        if self._tokens is not None:
            principal = self._tokens.principal(token)
            if principal is not None:
                return principal
        raise AuthenticationFailed(401, "the bearer token is not known")


PUBLIC_PATHS = frozenset({"/healthz", "/readyz"})
DISPATCHER_PATHS = frozenset({"/v1/dispatch", "/v1/reap", "/v1/facts"})
INTERACTIVE_PATHS = frozenset(
    {"/v1/connect", "/v1/disconnect", "/v1/sh", "/v1/sh/stream"}
)
READ_ONLY_PATHS = frozenset({"/v1/help", "/v1/openapi.json", "/v1/docs"})
READ_ONLY_PREFIXES = ("/v1/help/", "/v1/static/")
READ_METHODS = frozenset({"GET", "HEAD"})
DISPATCHERS = frozenset({DISPATCHER})
OPERATORS = frozenset({OPERATOR})
DISPATCHERS_AND_OPERATORS = frozenset({DISPATCHER, OPERATOR})


def required_roles(method: str, path: str) -> frozenset[str] | None:
    if path in PUBLIC_PATHS:
        return None
    if path in DISPATCHER_PATHS:
        return DISPATCHERS
    if path in INTERACTIVE_PATHS or path == "/mcp" or path.startswith("/mcp/"):
        return OPERATORS
    if path == "/v1/files" or path == "/v1/logs" or path.startswith("/v1/logs/"):
        return EVERYONE if method == "GET" else DISPATCHERS_AND_OPERATORS
    if method in READ_METHODS and (
        path in READ_ONLY_PATHS or path.startswith(READ_ONLY_PREFIXES)
    ):
        return EVERYONE
    return OPERATORS


class AuthenticationMiddleware:
    def __init__(self, application: ASGIApp, authenticator: Authenticator) -> None:
        self._application = application
        self._authenticator = authenticator
        self._mcp_sessions: OrderedDict[str, tuple[str, str]] = OrderedDict()

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http":
            await self._application(scope, receive, send)
            return
        path = scope["path"]
        method = scope["method"]
        roles = required_roles(method, path)
        if roles is None:
            await self._application(scope, receive, send)
            return
        try:
            principal = await self._authenticator.authenticate(scope)
        except AuthenticationFailed as failure:
            logger.warning(
                "refused a request",
                extra={
                    "path": path,
                    "method": method,
                    "status": failure.status,
                    "reason": failure.message,
                    "client": _client(scope),
                },
            )
            await _refuse(send, failure.status, failure.message)
            return
        if not roles & principal.roles:
            logger.warning(
                "refused a request its principal has no role for",
                extra={
                    "path": path,
                    "method": method,
                    "principal": principal.subject,
                    "roles": sorted(principal.roles),
                },
            )
            await _refuse(
                send, 403, f"{principal.subject} may not call {method} {path}"
            )
            return
        scope.setdefault("state", {})[PRINCIPAL] = principal
        if path == "/mcp" or path.startswith("/mcp/"):
            await self._mcp(scope, receive, send, principal)
            return
        await self._application(scope, receive, send)

    async def _mcp(
        self, scope: Scope, receive: Receive, send: Send, principal: Principal
    ) -> None:
        session_id = None
        for name, value in scope.get("headers", []):
            if name == b"mcp-session-id":
                session_id = value.decode("latin-1")
                break
        if session_id is not None:
            owner = self._mcp_sessions.get(session_id)
            if owner != (principal.method, principal.subject):
                logger.warning(
                    "refused an MCP request for a session another principal opened",
                    extra={"principal": principal.subject, "mcp_session": session_id},
                )
                await _refuse(send, 404, "no such MCP session")
                return
            self._mcp_sessions.move_to_end(session_id)
            if scope["method"] == "DELETE":
                del self._mcp_sessions[session_id]

        async def bind_session(message: Message) -> None:
            if message["type"] == "http.response.start" and session_id is None:
                for name, value in message.get("headers", []):
                    if name.lower() == b"mcp-session-id":
                        self._mcp_sessions[value.decode("latin-1")] = (
                            principal.method,
                            principal.subject,
                        )
                        while len(self._mcp_sessions) > MAX_MCP_SESSIONS:
                            self._mcp_sessions.popitem(last=False)
            await send(message)

        await self._application(scope, receive, bind_session)


def _client(scope: Scope) -> str | None:
    client = scope.get("client")
    return f"{client[0]}:{client[1]}" if client else None


async def _refuse(send: Send, status: int, message: str) -> None:
    body = json.dumps({"error": message}).encode()
    headers = [
        (b"content-type", b"application/json"),
        (b"content-length", str(len(body)).encode()),
    ]
    if status == 401:
        headers.append((b"www-authenticate", b"Bearer"))
    await send({"type": "http.response.start", "status": status, "headers": headers})
    await send({"type": "http.response.body", "body": body})
