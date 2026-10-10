from __future__ import annotations

import shutil
import subprocess

import pytest
from conftest import CLIENTS, Binding, Node, docker

GO_IMAGE = "golang:1.27.1-trixie"
NODE_IMAGE = "node:22.23.3-trixie-slim"
MAVEN_IMAGE = "maven:3.9.16-eclipse-temurin-21"


def redis_cli(binding: Binding, *arguments: str) -> str:
    finished = subprocess.run(
        [
            "redis-cli",
            "-h",
            "127.0.0.1",
            "-p",
            str(binding.port),
            "--no-raw",
            *arguments,
        ],
        capture_output=True,
        text=True,
        timeout=30,
    )
    return (finished.stdout + finished.stderr).strip()


def test_redis_cli(binding):
    if shutil.which("redis-cli") is None:
        pytest.skip("redis-cli is not on PATH")
    assert redis_cli(binding, "PING") == "PONG"
    assert redis_cli(binding, "PING", "hello") == '"hello"'
    assert redis_cli(binding, "COMMAND", "DOCS") == "(empty array)"
    assert redis_cli(binding, "SET", "cli.one", "1") == "OK"
    assert redis_cli(binding, "GET", "cli.one") == '"1"'
    assert redis_cli(binding, "GET", "cli.missing") == "(nil)"
    assert redis_cli(binding, "MSET", "cli.two", "2", "cli.three", "3") == "OK"
    assert (
        redis_cli(binding, "MGET", "cli.one", "cli.two", "cli.missing")
        == '1) "1"\n2) "2"\n3) (nil)'
    )
    assert (
        redis_cli(binding, "EXISTS", "cli.one", "cli.two", "cli.missing")
        == "(integer) 2"
    )
    assert redis_cli(binding, "TYPE", "cli.one") == "string"
    assert redis_cli(binding, "TYPE", "cli.missing") == "none"
    assert redis_cli(binding, "TYPE", "logs.overwritten") == "list"
    assert redis_cli(binding, "GET", "logs.overwritten").startswith("(error) WRONGTYPE")
    assert redis_cli(binding, "GET", "dusk.namespace_id").strip('"').isdigit()
    assert (
        redis_cli(binding, "KEYS", "cli.*")
        == '1) "cli.one"\n2) "cli.three"\n3) "cli.two"'
    )
    scanned = redis_cli(binding, "--scan", "--pattern", "cli.*", "--count", "2")
    assert sorted(scanned.split()) == ['"cli.one"', '"cli.three"', '"cli.two"']
    assert redis_cli(binding, "SET", "dusk.hostname", "redis-cli") == "OK"
    assert redis_cli(binding, "GET", "dusk.hostname") == '"redis-cli"'
    assert "not supported" in redis_cli(binding, "SET", "cli.expiring", "v", "EX", "10")
    assert (
        redis_cli(binding, "DEL", "cli.one", "cli.two", "cli.missing") == "(integer) 2"
    )
    assert redis_cli(binding, "EXISTS", "cli.one") == "(integer) 0"
    assert redis_cli(binding, "QUIT") == "OK"
    assert '"proto"\n 6) (integer) 2' in redis_cli(binding, "HELLO", "2")


def test_redis_py(binding):
    redis = pytest.importorskip("redis")
    client = redis.Redis(host="127.0.0.1", port=binding.port)
    assert client.ping() is True
    connection = client.connection_pool.get_connection()
    connection.send_command("PING", "hello")
    assert connection.read_response() == b"hello"
    client.connection_pool.release(connection)
    assert client.set("py.one", "1") is True
    assert client.get("py.one") == b"1"
    assert client.get("py.missing") is None
    assert client.mset({"py.two": "2", "py.three": "3"}) is True
    assert client.mget("py.one", "py.two", "py.missing") == [b"1", b"2", None]
    assert client.exists("py.one", "py.two", "py.missing") == 2
    assert client.type("py.one") == b"string"
    assert client.type("py.missing") == b"none"
    assert client.type("logs.overwritten") == b"list"
    with pytest.raises(redis.ResponseError, match="WRONGTYPE"):
        client.get("logs.overwritten")
    assert client.get("dusk.namespace_id").isdigit()
    assert sorted(client.keys("py.*")) == [b"py.one", b"py.three", b"py.two"]
    assert sorted(client.scan_iter(match="py.*", count=2)) == [
        b"py.one",
        b"py.three",
        b"py.two",
    ]
    assert client.set("dusk.hostname", "redis-py") is True
    assert client.get("dusk.hostname") == b"redis-py"
    with pytest.raises(redis.ResponseError, match="not supported"):
        client.set("py.expiring", "v", ex=10)
    assert client.delete("py.one", "py.two", "py.missing") == 2
    assert client.exists("py.one") == 0
    assert client.execute_command("QUIT") is True
    client.close()


def run_client(name: str, image: str, caches, command: str, binding: Binding) -> None:
    finished = docker(
        image,
        [f"{CLIENTS / name}:/src:ro", f"{caches}:/cache"],
        command.format(address=binding.address, prefix=f"{name}."),
    )
    print(finished.stdout, finished.stderr)
    assert finished.returncode == 0, finished.stdout + finished.stderr


def test_go_redis(binding, caches):
    run_client(
        "go-redis",
        GO_IMAGE,
        caches,
        "export GOPATH=/cache/go GOCACHE=/cache/go-build GOTOOLCHAIN=local GOFLAGS=-mod=readonly"
        " && cd /src && go build -o /tmp/client . && /tmp/client {address} {prefix}",
        binding,
    )


def test_ioredis(binding, caches):
    run_client(
        "ioredis",
        NODE_IMAGE,
        caches,
        "cp -r /src /tmp/client && cd /tmp/client"
        " && npm ci --cache /cache/npm --no-audit --no-fund --loglevel=error"
        " && node main.js {address} {prefix}",
        binding,
    )


def test_jedis(binding, caches):
    run_client(
        "jedis",
        MAVEN_IMAGE,
        caches,
        "export MAVEN_CONFIG=/cache/m2 && cp -r /src /tmp/client && cd /tmp/client"
        " && mvn -q -Dmaven.repo.local=/cache/m2 dependency:copy-dependencies -DoutputDirectory=lib"
        " && java -cp 'lib/*' Main.java {address} {prefix}",
        binding,
    )


def test_a_persistent_key_written_over_redis_survives_a_restart(tmp_path):
    redis = pytest.importorskip("redis")
    file = tmp_path / "kvs"
    node = Node(file)
    persistent = Binding(node, "--persistent")
    plain = Binding(node)
    try:
        assert redis.Redis(port=persistent.port).set("restart.kept", "survived") is True
        assert redis.Redis(port=plain.port).set("restart.lost", "gone") is True
        assert "persistent" in node.run("kvs scan")
    finally:
        persistent.stop()
        plain.stop()
        node.crash()
    assert b"survived" not in file.read_bytes()

    node = Node(file)
    binding = Binding(node)
    try:
        client = redis.Redis(port=binding.port)
        assert client.get("restart.kept") == b"survived"
        assert client.get("restart.lost") is None
    finally:
        binding.stop()
        node.crash()
