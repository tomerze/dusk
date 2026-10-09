from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest
from pydantic import ValidationError

from dawn.config import Settings, load, split_host_port

VALID = {
    "tls": {
        "certificate": "/etc/dawn/tls/server.crt",
        "key": "/etc/dawn/tls/server.key",
    },
    "auth": {"tokens_file": "/etc/dawn/secrets/tokens.toml"},
    "kafka": {"allow_plaintext": True},
}


def settings(**overrides) -> Settings:
    values: dict[str, Any] = {key: dict(value) for key, value in VALID.items()}
    for key, value in overrides.items():
        if isinstance(value, dict) and isinstance(values.get(key), dict):
            values[key].update(value)
        else:
            values[key] = value
    return Settings(**values)


def problems(**overrides) -> str:
    with pytest.raises(ValidationError) as raised:
        settings(**overrides)
    return str(raised.value)


@pytest.fixture(autouse=True)
def clean_environment(monkeypatch: pytest.MonkeyPatch):
    import os

    for name in list(os.environ):
        if name.upper().startswith("DAWN"):
            monkeypatch.delenv(name)


def test_the_defaults_are_the_ones_the_spec_ships():
    loaded = settings()

    assert loaded.listen == "0.0.0.0:8443"
    assert loaded.metrics_listen == "0.0.0.0:9101"
    assert loaded.principals == {"twilight-*": "dispatcher"}
    assert loaded.limits.max_concurrent_uploads == 64
    assert loaded.limits.max_file_bytes * 3 == loaded.limits.max_staged_bytes
    assert loaded.limits.max_staged_bytes < 256 * 1024 * 1024
    assert loaded.limits.max_output_bytes == 1048576
    assert loaded.kafka.topics.process_results == "dusk.process-results"
    assert loaded.kafka.topics.process_output == "dusk.process-output"
    assert loaded.kafka.topics.files == "dusk.files"


def test_environment_variables_override_nested_keys(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("DAWN__LIMITS__MAX_NODE_SESSIONS", "250")
    monkeypatch.setenv("DAWN__NIGHTFALL__SERVER_NAME_SUFFIX", "fleet.example.org")
    monkeypatch.setenv(
        "DAWN__PRINCIPALS", '{"twilight-*": "dispatcher", "ops-*": "operator"}'
    )

    values: dict[str, Any] = {key: dict(value) for key, value in VALID.items()}
    loaded = Settings(**values)

    assert loaded.limits.max_node_sessions == 250
    assert loaded.nightfall.server_name_suffix == "fleet.example.org"
    assert loaded.principals["ops-*"] == "operator"


def test_a_toml_file_is_read_and_the_environment_wins_over_it(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    file = tmp_path / "dawn.toml"
    file.write_text(
        'instance = "dawn-3"\n'
        "[tls]\n"
        'certificate = "/tls/server.crt"\n'
        'key = "/tls/server.key"\n'
        "[auth]\n"
        'tokens_file = "/secrets/tokens.toml"\n'
        "[kafka]\n"
        "allow_plaintext = true\n"
        "[limits]\n"
        "max_log_streams = 8\n"
        "max_node_sessions = 100\n"
    )
    monkeypatch.setenv("DAWN__LIMITS__MAX_NODE_SESSIONS", "120")

    loaded = load(file)

    assert loaded.instance == "dawn-3"
    assert loaded.limits.max_log_streams == 8
    assert loaded.limits.max_node_sessions == 120


def test_the_file_named_by_dawn_config_is_read(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    file = tmp_path / "elsewhere.toml"
    file.write_text(
        '[tls]\ncertificate = "/c"\nkey = "/k"\n[auth]\ntokens_file = "/t"\n'
        "[kafka]\nallow_plaintext = true\n"
    )
    monkeypatch.setenv("DAWN_CONFIG", str(file))

    assert load().tls.certificate == Path("/c")


def test_a_missing_configuration_file_is_refused(tmp_path: Path):
    with pytest.raises(ValueError, match="does not exist"):
        load(tmp_path / "absent.toml")


def test_an_unknown_key_is_refused_rather_than_ignored():
    assert "extra" in problems(limits={"max_node_session": 10}).lower()


def test_dawn_refuses_to_serve_without_a_server_certificate():
    assert "dawn serves HTTPS only" in problems(tls={"certificate": None})


def test_someone_has_to_be_able_to_authenticate():
    message = problems(auth={"tokens_file": None})

    assert "no caller can authenticate" in message


def test_mutual_tls_alone_is_enough_to_authenticate():
    loaded = settings(
        auth={"tokens_file": None}, tls={"client_ca": "/pki/internal-ca.crt"}
    )

    assert loaded.auth.tokens_file is None


def test_oidc_needs_both_the_issuer_and_the_audience():
    assert "set together" in problems(auth={"oidc_issuer": "https://id.example.org"})


def test_oidc_issuer_must_be_https():
    message = problems(
        auth={"oidc_issuer": "http://id.example.org", "oidc_audience": "dawn"}
    )

    assert "is not an https URL" in message


def test_plaintext_kafka_is_refused_unless_allowed():
    assert "allow_plaintext" in problems(kafka={"allow_plaintext": False})
    secure = settings(
        kafka={"allow_plaintext": False, "properties": {"security_protocol": "SSL"}}
    )

    assert secure.kafka.properties["security_protocol"] == "SSL"


@pytest.mark.parametrize(
    "override",
    [
        {"listen": "8443"},
        {"metrics_listen": "0.0.0.0:99999"},
        {"nightfall": {"default_inner_address": "nightfall"}},
        {"collector": {"endpoint": "otel-collector"}},
        {"collector": {"allowed_endpoints": ["collector:4317", "not an endpoint"]}},
        {"nightfall": {"allowed_inner_addresses": ["nightfall-*:x"]}},
        {"nightfall": {"server_name_suffix": "fleet example"}},
        {"s3": {"bucket": "Not_A_Bucket"}},
        {"s3": {"access_key_file": "/secrets/s3-access"}},
        {"s3": {"endpoint": "rgw:8080"}},
        {"otlp": {"endpoint": "otel-collector:4318"}},
        {"kafka": {"topics": {"process_results": "has spaces"}}},
        {"instance": "dawn/0"},
        {"limits": {"process_timeout_default": 100, "process_timeout_max": 10}},
        {"limits": {"max_node_sessions": 0}},
        {"limits": {"max_file_bytes": 300, "max_staged_bytes": 200}},
    ],
)
def test_a_malformed_value_is_refused(override: dict):
    problems(**override)


def test_every_problem_is_reported_at_once():
    message = problems(tls={"certificate": None}, s3={"bucket": "x"})

    assert "dawn serves HTTPS only" in message
    assert "is not a bucket name" in message


def test_the_default_inner_address_and_the_allowed_patterns_are_allowed():
    loaded = settings(
        nightfall={
            "default_inner_address": "nightfall-inner:8444",
            "allowed_inner_addresses": ["nightfall-*.nightfall-inner.dusk.svc:8444"],
        }
    )

    assert loaded.inner_address_allowed("nightfall-inner:8444")
    assert loaded.inner_address_allowed("nightfall-2.nightfall-inner.dusk.svc:8444")
    assert not loaded.inner_address_allowed("nightfall-2.nightfall-inner.dusk.svc:9999")
    assert not loaded.inner_address_allowed("attacker.example.org:8444")
    assert not loaded.inner_address_allowed(
        "nightfall-2.attacker.example.org.nightfall-inner.dusk.svc:8444"
    )


def test_only_listed_collector_endpoints_are_allowed():
    loaded = settings(
        collector={
            "endpoint": "otel-collector:4317",
            "allowed_endpoints": ["tenant-a:4317"],
        }
    )

    assert loaded.collector_endpoint_allowed("otel-collector:4317")
    assert loaded.collector_endpoint_allowed("tenant-a:4317")
    assert not loaded.collector_endpoint_allowed("tenant-b:4317")


@pytest.mark.parametrize(
    ("address", "expected"),
    [
        ("nightfall:8444", ("nightfall", 8444)),
        ("10.0.0.1:1", ("10.0.0.1", 1)),
        ("[fd00::1]:8444", ("fd00::1", 8444)),
    ],
)
def test_host_and_port_are_split(address: str, expected: tuple[str, int]):
    assert split_host_port(address) == expected


@pytest.mark.parametrize(
    "address", ["nightfall", ":8444", "nightfall:0", "nightfall:65536", "a b:1"]
)
def test_a_malformed_address_is_refused(address: str):
    with pytest.raises(ValueError):
        split_host_port(address)
