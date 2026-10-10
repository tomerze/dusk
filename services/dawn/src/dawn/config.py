from __future__ import annotations

import os
import re
import socket
from pathlib import Path
from typing import Literal
from urllib.parse import urlsplit

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator
from pydantic_settings import (
    BaseSettings,
    PydanticBaseSettingsSource,
    SettingsConfigDict,
    TomlConfigSettingsSource,
)

DEFAULT_CONFIG_FILE = Path("/etc/dawn/dawn.toml")
CONFIG_FILE_VARIABLE = "DAWN_CONFIG"

Role = Literal["dispatcher", "operator", "viewer"]

HOST_PORT = re.compile(r"^(\[[0-9A-Fa-f:.]+\]|[A-Za-z0-9.-]+):([0-9]{1,5})$")
HOST_PORT_PATTERN = re.compile(r"^(\[[0-9A-Fa-f:.]+\]|[A-Za-z0-9.*-]+):([0-9]{1,5})$")
BUCKET = re.compile(r"^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$")
INSTANCE = re.compile(r"^[^/\s]{1,253}$")
TOPIC = re.compile(r"^[A-Za-z0-9._-]{1,249}$")
SECURE_KAFKA_PROTOCOLS = frozenset({"SSL", "SASL_SSL"})


def split_host_port(
    address: str, pattern: re.Pattern[str] = HOST_PORT
) -> tuple[str, int]:
    match = pattern.match(address)
    if match is None:
        raise ValueError(f"{address!r} is not host:port")
    port = int(match.group(2))
    if not 0 < port < 65536:
        raise ValueError(f"{address!r} has port {port}, outside 1-65535")
    return match.group(1).removeprefix("[").removesuffix("]"), port


class Section(BaseModel):
    model_config = ConfigDict(extra="forbid")


class TlsSettings(Section):
    certificate: Path | None = None
    key: Path | None = None
    client_ca: Path | None = None


class AuthSettings(Section):
    tokens_file: Path | None = None
    oidc_issuer: str | None = None
    oidc_audience: str | None = None
    role_claim: str = "groups"
    role_map: dict[str, Role] = Field(default_factory=dict)


class NightfallSettings(Section):
    default_inner_address: str = "nightfall-inner:8444"
    allowed_inner_addresses: list[str] = Field(default_factory=list)
    server_name_suffix: str = "fleet.dusk.example"
    ca: Path = Path("/etc/dawn/pki/internal-ca.crt")
    certificate: Path = Path("/etc/dawn/tls/client.crt")
    key: Path = Path("/etc/dawn/tls/client.key")
    connect_timeout_seconds: float = Field(default=10.0, gt=0, le=300)


class KafkaTopics(Section):
    process_results: str = "dusk.process-results"
    process_output: str = "dusk.process-output"
    files: str = "dusk.files"


class KafkaSettings(Section):
    brokers: str = "kafka:9092"
    allow_plaintext: bool = False
    properties: dict[str, str | int | float | bool] = Field(default_factory=dict)
    topics: KafkaTopics = Field(default_factory=KafkaTopics)


class S3Settings(Section):
    endpoint: str | None = None
    region: str = "us-east-1"
    bucket: str = "dusk-files"
    access_key_file: Path | None = None
    secret_key_file: Path | None = None
    ca: Path | None = None


class OtlpSettings(Section):
    endpoint: str | None = None


class CollectorSettings(Section):
    endpoint: str = "otel-collector:4317"
    allowed_endpoints: list[str] = Field(default_factory=list)


class LimitSettings(Section):
    max_node_sessions: int = Field(default=10000, ge=1)
    max_log_streams: int = Field(default=256, ge=1)
    max_concurrent_uploads: int = Field(default=64, ge=1)
    max_staged_bytes: int = Field(default=201326592, ge=1)
    max_interactive_sessions: int = Field(default=256, ge=1)
    interactive_session_seconds: int = Field(default=28800, ge=1)
    max_work_per_node: int = Field(default=32, ge=1, le=1024)
    max_queued_script_bytes: int = Field(default=268435456, ge=1048576)
    process_timeout_default: int = Field(default=900, ge=1)
    process_timeout_max: int = Field(default=86400, ge=1)
    max_log_stream_seconds: int = Field(default=3600, ge=1)
    max_file_bytes: int = Field(default=67108864, ge=1)
    max_output_bytes: int = Field(default=1048576, ge=1)


class Settings(BaseSettings):
    model_config = SettingsConfigDict(
        env_prefix="DAWN__",
        env_nested_delimiter="__",
        extra="forbid",
        case_sensitive=False,
    )

    listen: str = "0.0.0.0:8443"
    metrics_listen: str = "0.0.0.0:9101"
    instance: str = Field(default_factory=socket.gethostname)
    drain_seconds: int = Field(default=300, ge=0, le=86400)
    log_level: Literal["debug", "info", "warning", "error"] = "info"
    output_key_file: Path = Path("/etc/dawn/secrets/output.key")
    principals: dict[str, Role] = Field(
        default_factory=lambda: {"twilight-*": "dispatcher"}
    )
    tls: TlsSettings = Field(default_factory=TlsSettings)
    auth: AuthSettings = Field(default_factory=AuthSettings)
    nightfall: NightfallSettings = Field(default_factory=NightfallSettings)
    kafka: KafkaSettings = Field(default_factory=KafkaSettings)
    s3: S3Settings = Field(default_factory=S3Settings)
    otlp: OtlpSettings = Field(default_factory=OtlpSettings)
    collector: CollectorSettings = Field(default_factory=CollectorSettings)
    limits: LimitSettings = Field(default_factory=LimitSettings)

    @classmethod
    def settings_customise_sources(
        cls,
        settings_cls: type[BaseSettings],
        init_settings: PydanticBaseSettingsSource,
        env_settings: PydanticBaseSettingsSource,
        dotenv_settings: PydanticBaseSettingsSource,
        file_secret_settings: PydanticBaseSettingsSource,
    ) -> tuple[PydanticBaseSettingsSource, ...]:
        return (init_settings, env_settings, TomlConfigSettingsSource(settings_cls))

    @field_validator("listen", "metrics_listen")
    @classmethod
    def _address(cls, address: str) -> str:
        split_host_port(address)
        return address

    @field_validator("instance")
    @classmethod
    def _instance(cls, instance: str) -> str:
        if not INSTANCE.match(instance):
            raise ValueError(
                f"{instance!r} is not an instance name: no slash or whitespace"
            )
        return instance

    @model_validator(mode="after")
    def _consistent(self) -> Settings:
        problems: list[str] = []

        if self.tls.certificate is None or self.tls.key is None:
            problems.append(
                "tls.certificate and tls.key are required: dawn serves HTTPS only"
            )

        oidc = (self.auth.oidc_issuer, self.auth.oidc_audience)
        if any(oidc) and not all(oidc):
            problems.append(
                "auth.oidc_issuer and auth.oidc_audience are set together or not at all"
            )
        if self.auth.oidc_issuer is not None and not _is_url(
            self.auth.oidc_issuer, {"https"}
        ):
            problems.append(
                f"auth.oidc_issuer {self.auth.oidc_issuer!r} is not an https URL"
            )
        if (
            self.auth.tokens_file is None
            and self.auth.oidc_issuer is None
            and (self.tls.client_ca is None or not self.principals)
        ):
            problems.append(
                "no caller can authenticate: set auth.tokens_file, auth.oidc_issuer, "
                "or tls.client_ca with principals"
            )

        for name, address in [
            ("nightfall.default_inner_address", self.nightfall.default_inner_address),
            ("collector.endpoint", self.collector.endpoint),
        ]:
            try:
                split_host_port(address)
            except ValueError as failure:
                problems.append(f"{name}: {failure}")
        for address in self.nightfall.allowed_inner_addresses:
            try:
                split_host_port(address, HOST_PORT_PATTERN)
            except ValueError as failure:
                problems.append(f"nightfall.allowed_inner_addresses: {failure}")
        for address in self.collector.allowed_endpoints:
            try:
                split_host_port(address)
            except ValueError as failure:
                problems.append(f"collector.allowed_endpoints: {failure}")
        if not re.fullmatch(r"[A-Za-z0-9.-]+", self.nightfall.server_name_suffix):
            problems.append(
                f"nightfall.server_name_suffix {self.nightfall.server_name_suffix!r} is not a DNS name"
            )

        if not self.kafka.brokers.strip():
            problems.append("kafka.brokers is empty")
        protocol = str(
            self.kafka.properties.get("security_protocol", "PLAINTEXT")
        ).upper()
        if protocol not in SECURE_KAFKA_PROTOCOLS and not self.kafka.allow_plaintext:
            problems.append(
                f"kafka.properties.security_protocol is {protocol}: set SSL or SASL_SSL, "
                "or kafka.allow_plaintext = true for development"
            )
        for topic in (
            self.kafka.topics.process_results,
            self.kafka.topics.process_output,
            self.kafka.topics.files,
        ):
            if not TOPIC.match(topic):
                problems.append(f"kafka topic {topic!r} is not a topic name")

        if not BUCKET.match(self.s3.bucket):
            problems.append(f"s3.bucket {self.s3.bucket!r} is not a bucket name")
        if self.s3.endpoint is not None and not _is_url(
            self.s3.endpoint, {"http", "https"}
        ):
            problems.append(f"s3.endpoint {self.s3.endpoint!r} is not an http(s) URL")
        if (self.s3.access_key_file is None) != (self.s3.secret_key_file is None):
            problems.append(
                "s3.access_key_file and s3.secret_key_file are set together or not at all"
            )

        if self.otlp.endpoint is not None and not _is_url(
            self.otlp.endpoint, {"http", "https"}
        ):
            problems.append(
                f"otlp.endpoint {self.otlp.endpoint!r} is not an http(s) URL"
            )

        if self.limits.process_timeout_default > self.limits.process_timeout_max:
            problems.append(
                "limits.process_timeout_default exceeds limits.process_timeout_max"
            )
        if self.limits.max_file_bytes > self.limits.max_staged_bytes:
            problems.append("limits.max_file_bytes exceeds limits.max_staged_bytes")

        if problems:
            raise ValueError("; ".join(problems))
        return self

    def inner_address_allowed(self, address: str) -> bool:
        if address == self.nightfall.default_inner_address:
            return True
        return any(
            _matches(pattern, address)
            for pattern in self.nightfall.allowed_inner_addresses
        )

    def collector_endpoint_allowed(self, endpoint: str) -> bool:
        return (
            endpoint == self.collector.endpoint
            or endpoint in self.collector.allowed_endpoints
        )


def _is_url(text: str, schemes: set[str]) -> bool:
    parts = urlsplit(text)
    return parts.scheme in schemes and bool(parts.netloc)


def _matches(pattern: str, text: str) -> bool:
    expression = "[A-Za-z0-9-]+".join(re.escape(piece) for piece in pattern.split("*"))
    return re.fullmatch(expression, text) is not None


def load(path: Path | None = None) -> Settings:
    if path is None:
        configured = os.environ.get(CONFIG_FILE_VARIABLE)
        if configured:
            path = Path(configured)
        elif DEFAULT_CONFIG_FILE.exists():
            path = DEFAULT_CONFIG_FILE
    if path is not None and not path.is_file():
        raise ValueError(f"the configuration file {path} does not exist")

    class FileSettings(Settings):
        model_config = SettingsConfigDict(toml_file=path)

    return FileSettings()
