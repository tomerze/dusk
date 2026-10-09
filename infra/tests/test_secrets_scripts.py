import base64
import json
import os
import shutil
import subprocess
import sys
import threading
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import pytest

REPOSITORY = Path(
    os.environ.get("REPOSITORY_DIRECTORY") or Path(__file__).resolve().parents[2]
)
SCRIPTS = REPOSITORY / "infra" / "k8s" / "base" / "secrets-init"
GENERATE = SCRIPTS / "generate.sh"
KUBERNETES = SCRIPTS / "kubernetes.sh"
NAMESPACE = "dusk"

pytestmark = pytest.mark.skipif(
    not GENERATE.exists()
    or any(shutil.which(tool) is None for tool in ("sh", "bash", "curl", "jq")),
    reason="needs the secrets-init scripts, sh, bash, curl and jq",
)

STEP = """
import json
import secrets
import string
import sys
from pathlib import Path

arguments = sys.argv[1:]


def option(name):
    return arguments[arguments.index(name) + 1]


def key(kind):
    return {"kty": "OKP", "crv": "Ed25519", "kid": secrets.token_hex(8), "kind": kind}


if arguments[:2] == ["crypto", "rand"]:
    length = int(arguments[-1])
    alphabet = {
        "hex": "0123456789abcdef",
        "alphanumeric": string.ascii_letters + string.digits,
        "upper": string.ascii_uppercase,
    }[option("--format")]
    sys.stdout.write("".join(secrets.choice(alphabet) for _ in range(length)))
elif arguments[:3] == ["crypto", "jwk", "create"]:
    public = key("public")
    Path(arguments[3]).write_text(json.dumps(public))
    Path(arguments[4]).write_text(json.dumps(public | {"d": secrets.token_hex(32)}))
elif arguments[:3] == ["crypto", "jwk", "keyset"]:
    keyset = Path(arguments[4])
    keys = json.loads(keyset.read_text())["keys"] if keyset.exists() else []
    keys.append(json.loads(sys.stdin.read()))
    keyset.write_text(json.dumps({"keys": keys}))
elif arguments[:2] == ["crypto", "keypair"]:
    Path(arguments[2]).write_text("public " + secrets.token_hex(32))
    Path(arguments[3]).write_text("private " + secrets.token_hex(32))
else:
    sys.exit("unexpected step arguments: " + " ".join(arguments))
"""


@pytest.fixture
def step(tmp_path: Path) -> dict[str, str]:
    directory = tmp_path / "bin"
    directory.mkdir()
    stub = directory / "step"
    stub.write_text("#!" + sys.executable + "\n" + STEP)
    stub.chmod(0o755)
    return {**os.environ, "PATH": f"{directory}{os.pathsep}{os.environ['PATH']}"}


def generate(output: Path, environment: dict[str, str]) -> str:
    finished = subprocess.run(
        ["sh", str(GENERATE), str(output)],
        env=environment,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert finished.returncode == 0, finished.stderr
    return finished.stdout


def snapshot(directory: Path) -> dict[str, bytes]:
    return {
        str(path.relative_to(directory)): path.read_bytes()
        for path in sorted(directory.rglob("*"))
        if path.is_file()
    }


def test_generate_keeps_every_secret_that_exists(
    tmp_path: Path, step: dict[str, str]
) -> None:
    output = tmp_path / "secrets"
    first = generate(output, step)
    groups = sorted(path.name for path in output.iterdir())
    assert "secret nightfall-ledger created" in first
    assert not [name for name in groups if name.startswith(".")]
    before = snapshot(output)

    second = generate(output, step)

    assert snapshot(output) == before
    assert "created" not in second
    assert second.count("exists, kept") == len(groups)


def test_generate_derives_the_verify_group_from_the_ledger_keys(
    tmp_path: Path, step: dict[str, str]
) -> None:
    output = tmp_path / "secrets"
    generate(output, step)
    ledger = (output / "nightfall-ledger" / "ledger-verify-jwks.json").read_bytes()
    verify = output / "nightfall-ledger-verify"
    assert sorted(path.name for path in verify.iterdir()) == ["ledger-verify-jwks.json"]
    assert (verify / "ledger-verify-jwks.json").read_bytes() == ledger

    verify.chmod(0o755)
    shutil.rmtree(verify)
    before = snapshot(output)
    log = generate(output, step)

    assert "secret nightfall-ledger-verify created" in log
    assert (verify / "ledger-verify-jwks.json").read_bytes() == ledger
    assert {
        name: content
        for name, content in snapshot(output).items()
        if not name.startswith("nightfall-ledger-verify/")
    } == before


class FakeApi:
    def __init__(self) -> None:
        self.objects: dict[tuple[str, str], dict[str, Any]] = {}
        self.requests: list[tuple[str, str]] = []
        self.version = 0

    def store(self, collection: str, document: dict[str, Any]) -> dict[str, Any]:
        self.version += 1
        document["metadata"]["resourceVersion"] = str(self.version)
        self.objects[(collection, document["metadata"]["name"])] = document
        return document

    def handle(
        self, method: str, path: str, body: dict[str, Any] | None
    ) -> tuple[int, dict[str, Any]]:
        self.requests.append((method, path))
        prefix = f"/api/v1/namespaces/{NAMESPACE}/"
        assert path.startswith(prefix), path
        collection, _, name = path.removeprefix(prefix).partition("/")
        if method == "POST":
            assert body is not None
            if (collection, body["metadata"]["name"]) in self.objects:
                return 409, {"reason": "AlreadyExists"}
            return 201, self.store(collection, body)
        current = self.objects.get((collection, name))
        if current is None:
            return 404, {"reason": "NotFound"}
        if method == "GET":
            return 200, current
        assert method == "PUT" and body is not None
        if (
            body["metadata"].get("resourceVersion")
            != current["metadata"]["resourceVersion"]
        ):
            return 409, {"reason": "Conflict"}
        return 200, self.store(collection, body)


@pytest.fixture
def api(tmp_path: Path) -> Iterator[tuple[FakeApi, dict[str, str]]]:
    fake = FakeApi()

    class Handler(BaseHTTPRequestHandler):
        def respond(self) -> None:
            length = int(self.headers.get("Content-Length") or 0)
            body = json.loads(self.rfile.read(length)) if length else None
            assert self.headers["Authorization"] == "Bearer token"
            status, document = fake.handle(self.command, self.path, body)
            payload = json.dumps(document).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        do_GET = do_POST = do_PUT = respond

        def log_message(self, format: str, *arguments: Any) -> None:
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    account = tmp_path / "account"
    account.mkdir()
    (account / "token").write_text("token")
    (account / "namespace").write_text(NAMESPACE)
    (account / "ca.crt").write_text("")
    environment = {
        **os.environ,
        "KUBERNETES_API": f"http://127.0.0.1:{server.server_address[1]}",
        "KUBERNETES_ACCOUNT": str(account),
    }
    try:
        yield fake, environment
    finally:
        server.shutdown()
        thread.join()


def kubernetes(
    environment: dict[str, str], *arguments: str
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["bash", str(KUBERNETES), *arguments],
        env=environment,
        capture_output=True,
        text=True,
        timeout=60,
    )


def groups(root: Path, contents: dict[str, dict[str, str]]) -> Path:
    for name, files in contents.items():
        directory = root / name
        directory.mkdir(parents=True)
        for key, value in files.items():
            (directory / key).write_text(value)
    return root


def secret(name: str, data: dict[str, str]) -> dict[str, Any]:
    return {
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {"name": name},
        "type": "Opaque",
        "data": {
            key: base64.b64encode(value.encode()).decode()
            for key, value in data.items()
        },
    }


def test_create_never_replaces_a_secret(
    tmp_path: Path, api: tuple[FakeApi, dict[str, str]]
) -> None:
    fake, environment = api
    fake.store("secrets", secret("kept", {"password": "old"}))
    source = groups(
        tmp_path / "source", {"kept": {"password": "new"}, "added": {"password": "x"}}
    )

    finished = kubernetes(environment, "create", "secret", str(source))

    assert finished.returncode == 0, finished.stderr
    assert "secret kept exists, kept" in finished.stdout
    assert [method for method, _ in fake.requests] == ["POST", "POST"]
    assert (
        fake.objects[("secrets", "kept")]["data"]
        == secret("kept", {"password": "old"})["data"]
    )
    assert fake.objects[("secrets", "added")]["data"] == {"password": "eA=="}


def test_create_new_fails_when_the_secret_exists(
    tmp_path: Path, api: tuple[FakeApi, dict[str, str]]
) -> None:
    fake, environment = api
    fake.store("secrets", secret("step-ca-root-key", {"root_ca_key": "old"}))
    source = groups(tmp_path / "source", {"step-ca-root-key": {"root_ca_key": "new"}})

    finished = kubernetes(environment, "create-new", "secret", str(source))

    assert finished.returncode != 0
    assert "HTTP 409" in finished.stderr
    assert [method for method, _ in fake.requests] == ["POST"]
    assert fake.objects[("secrets", "step-ca-root-key")]["data"] == {
        "root_ca_key": base64.b64encode(b"old").decode()
    }


def test_apply_replaces_only_changed_data(
    tmp_path: Path, api: tuple[FakeApi, dict[str, str]]
) -> None:
    fake, environment = api
    fake.store("secrets", secret("same", {"tls.crt": "a", "tls.key": "b"}))
    fake.store("secrets", secret("changed", {"tls.crt": "a", "tls.key": "b"}))
    version = fake.objects[("secrets", "changed")]["metadata"]["resourceVersion"]
    source = groups(
        tmp_path / "source",
        {
            "changed": {"tls.crt": "c", "tls.key": "d"},
            "same": {"tls.crt": "a", "tls.key": "b"},
        },
    )

    finished = kubernetes(environment, "apply", "secret", str(source))

    assert finished.returncode == 0, finished.stderr
    assert "secret same is current" in finished.stdout
    assert "secret changed replaced" in finished.stdout
    assert [request for request in fake.requests if request[0] == "PUT"] == [
        ("PUT", f"/api/v1/namespaces/{NAMESPACE}/secrets/changed")
    ]
    changed = fake.objects[("secrets", "changed")]
    assert changed["type"] == "kubernetes.io/tls"
    assert (
        changed["data"] == secret("changed", {"tls.crt": "c", "tls.key": "d"})["data"]
    )
    assert changed["metadata"]["resourceVersion"] != version


def test_apply_writes_configmap_data_as_text(
    tmp_path: Path, api: tuple[FakeApi, dict[str, str]]
) -> None:
    fake, environment = api
    source = groups(tmp_path / "source", {"anchors": {"internal-ca.crt": "pem\n"}})

    finished = kubernetes(environment, "apply", "configmap", str(source))

    assert finished.returncode == 0, finished.stderr
    assert fake.objects[("configmaps", "anchors")]["data"] == {
        "internal-ca.crt": "pem\n"
    }


def test_fetch_writes_each_key_and_reports_a_missing_secret(
    tmp_path: Path, api: tuple[FakeApi, dict[str, str]]
) -> None:
    fake, environment = api
    fake.store("secrets", secret("ledger", {"ledger-signing.key": "key\n"}))

    found = kubernetes(
        environment, "fetch", "secret", "ledger", str(tmp_path / "ledger")
    )
    missing = kubernetes(
        environment, "fetch", "secret", "absent", str(tmp_path / "absent")
    )

    assert found.returncode == 0, found.stderr
    assert (tmp_path / "ledger" / "ledger-signing.key").read_text() == "key\n"
    assert missing.returncode == 3
    assert not (tmp_path / "absent").exists()
