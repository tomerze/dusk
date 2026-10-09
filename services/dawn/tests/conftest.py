from __future__ import annotations

import importlib.util
import pathlib
import sys
import types
from typing import Any

import pytest

PYTHON_SOURCE = pathlib.Path(__file__).resolve().parents[3] / "dusk/src/dusk_py/python"

if importlib.util.find_spec("dusk") is None:
    sys.path.insert(0, str(PYTHON_SOURCE))
    sys.modules["dusk.dusk"] = types.ModuleType("dusk.dusk")


@pytest.fixture
def anyio_backend() -> str:
    return "asyncio"


class CertificateAuthority:
    def __init__(self, name: str = "dusk internal test CA") -> None:
        import datetime

        from cryptography import x509
        from cryptography.hazmat.primitives import hashes
        from cryptography.hazmat.primitives.asymmetric import ec
        from cryptography.x509.oid import NameOID

        self.key = ec.generate_private_key(ec.SECP256R1())
        subject = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, name)])
        now = datetime.datetime.now(datetime.UTC)
        self.certificate = (
            x509.CertificateBuilder()
            .subject_name(subject)
            .issuer_name(subject)
            .public_key(self.key.public_key())
            .serial_number(x509.random_serial_number())
            .not_valid_before(now - datetime.timedelta(minutes=5))
            .not_valid_after(now + datetime.timedelta(days=1))
            .add_extension(
                x509.BasicConstraints(ca=True, path_length=None), critical=True
            )
            .add_extension(
                x509.SubjectKeyIdentifier.from_public_key(self.key.public_key()),
                critical=False,
            )
            .add_extension(
                x509.KeyUsage(
                    digital_signature=True,
                    content_commitment=False,
                    key_encipherment=False,
                    data_encipherment=False,
                    key_agreement=False,
                    key_cert_sign=True,
                    crl_sign=True,
                    encipher_only=False,
                    decipher_only=False,
                ),
                critical=True,
            )
            .sign(self.key, hashes.SHA256())
        )

    def issue(
        self,
        uris: tuple[str, ...] = (),
        dns_names: tuple[str, ...] = (),
        ip_addresses: tuple[str, ...] = (),
        server: bool = False,
    ):
        import datetime
        import ipaddress

        from cryptography import x509
        from cryptography.hazmat.primitives import hashes
        from cryptography.hazmat.primitives.asymmetric import ec
        from cryptography.x509.oid import ExtendedKeyUsageOID

        key = ec.generate_private_key(ec.SECP256R1())
        now = datetime.datetime.now(datetime.UTC)
        names: list[x509.GeneralName] = [
            x509.UniformResourceIdentifier(uri) for uri in uris
        ]
        names += [x509.DNSName(name) for name in dns_names]
        names += [
            x509.IPAddress(ipaddress.ip_address(address)) for address in ip_addresses
        ]
        builder = (
            x509.CertificateBuilder()
            .subject_name(x509.Name([]))
            .issuer_name(self.certificate.subject)
            .public_key(key.public_key())
            .serial_number(x509.random_serial_number())
            .not_valid_before(now - datetime.timedelta(minutes=5))
            .not_valid_after(now + datetime.timedelta(hours=24))
            .add_extension(
                x509.ExtendedKeyUsage(
                    [
                        ExtendedKeyUsageOID.SERVER_AUTH
                        if server
                        else ExtendedKeyUsageOID.CLIENT_AUTH
                    ]
                ),
                critical=False,
            )
            .add_extension(
                x509.SubjectKeyIdentifier.from_public_key(key.public_key()),
                critical=False,
            )
            .add_extension(
                x509.AuthorityKeyIdentifier.from_issuer_public_key(
                    self.key.public_key()
                ),
                critical=False,
            )
        )
        if names:
            builder = builder.add_extension(
                x509.SubjectAlternativeName(names), critical=True
            )
        return key, builder.sign(self.key, hashes.SHA256())

    def write(self, directory, stem: str, key, certificate) -> tuple[str, str]:
        from cryptography.hazmat.primitives import serialization

        key_path = directory / f"{stem}.key"
        certificate_path = directory / f"{stem}.crt"
        key_path.write_bytes(
            key.private_bytes(
                serialization.Encoding.PEM,
                serialization.PrivateFormat.PKCS8,
                serialization.NoEncryption(),
            )
        )
        certificate_path.write_bytes(
            certificate.public_bytes(serialization.Encoding.PEM)
        )
        return str(certificate_path), str(key_path)

    def write_root(self, directory) -> str:
        from cryptography.hazmat.primitives import serialization

        path = directory / "ca.crt"
        path.write_bytes(self.certificate.public_bytes(serialization.Encoding.PEM))
        return str(path)


def der(certificate) -> bytes:
    from cryptography.hazmat.primitives import serialization

    return certificate.public_bytes(serialization.Encoding.DER)


DEVICE_ID = "0123456789abcdef0123456789abcdef"
INSTALLATION_ID = "fedcba9876543210fedcba9876543210"
NAMESPACE_ID = "00000000000000aa"
CAMPAIGN_ID = "0b6b3d2a-1c4e-4f5a-9b8c-7d6e5f4a3b2c"
PID = 0x9E3779B97F4A7C15
KVS_TYPE_ID = "0x84e09148e9d394f3"
PS_TYPE_ID = "0xcef2c7c974bf44ec"
CP_TYPE_ID = "0xd82958034aade578"
LOGS_TYPE_ID = "0xb3d9f4a05c7e2186"
LOG_ATTRIBUTES_TYPE_ID = "0xdb2048b3069ea12b"
DEFAULT_SH_PID = 0xF2EFCE60E8C425D0


class FakeOutput:
    def __init__(self, values) -> None:
        self._values = iter(values)

    async def next_value(self):
        import asyncio

        while True:
            try:
                value = next(self._values)
            except StopIteration:
                raise StopAsyncIteration from None
            if isinstance(value, BaseException):
                raise value
            if isinstance(value, Wait):
                await asyncio.sleep(value.seconds)
                continue
            if isinstance(value, Step):
                value.action()
                continue
            return value


class Wait:
    def __init__(self, seconds: float) -> None:
        self.seconds = seconds


class Step:
    def __init__(self, action) -> None:
        self.action = action


def words_of(statement: str) -> list[str]:
    import re

    return [
        quoted_single or quoted_double or bare
        for quoted_single, quoted_double, bare in re.findall(
            r"'([^']+)'|\"([^\"]+)\"|([^\s'\"]+)", statement
        )
    ]


def statements_of(script: str) -> list[str]:
    statements, current, quote = [], "", None
    for character in script:
        if quote is None and character in "'\"":
            quote = character
        elif character == quote:
            quote = None
        if quote is None and character in ";\n":
            statements.append(current.strip())
            current = ""
            continue
        current += character
    statements.append(current.strip())
    return [statement for statement in statements if statement]


def script_span(pid: int, ended: bool = True) -> dict:
    fields: dict[str, object] = {
        "sequence": 1,
        "severity": "INFO",
        "name": "sh_exec",
        "startTimeUnixNano": 1,
        "attributes": {LOG_ATTRIBUTES_TYPE_ID: {"pid": f"{pid:x}"}},
    }
    if ended:
        fields["endTimeUnixNano"] = 2
    return {LOGS_TYPE_ID: fields}


class FakeConnection:
    def __init__(self, node: FakeNode, sh_server_pid: int) -> None:
        self.node = node
        self.sh_server_pid = sh_server_pid
        self.disconnected = False

    def sh(self, command: str) -> FakeOutput:
        self.node.calls.append(("sh", self.sh_server_pid, command))
        if self.disconnected:
            raise RuntimeError("Connection is closed")
        produced = self.node.scripts.get(command)
        if produced is not None:
            if callable(produced):
                produced = produced(self)
            if isinstance(produced, BaseException):
                return FakeOutput([produced])
            return FakeOutput(produced)
        statements = [words_of(statement) for statement in statements_of(command)]
        try:
            for words in statements:
                self.node.compile(words)
        except RuntimeError as failure:
            return FakeOutput([failure])
        steps: list = []
        for index, words in enumerate(statements):
            try:
                steps.extend(self.node.statement(self, words))
            except RuntimeError as failure:
                if index == len(statements) - 1:
                    steps.append(failure)
        return FakeOutput(steps)

    def disconnect(self) -> None:
        self.node.calls.append(("disconnect", self.sh_server_pid))
        if self.disconnected:
            raise RuntimeError("Already disconnected")
        self.disconnected = True


class FakeNode:
    def __init__(
        self,
        device_id: str = DEVICE_ID,
        installation_id: str = INSTALLATION_ID,
        namespace_id: str = NAMESPACE_ID,
    ) -> None:
        self.device_id = device_id
        self.installation_id = installation_id
        self.namespace_id = namespace_id
        self.kvs: dict[str, object] = {
            "dusk.version": "0.1.0",
            "dusk.device.id": "c" * 32,
            "dusk.os.locale": "en_US.UTF-8",
            "dusk.hostname": "node-1",
        }
        self.registered = set(self.kvs)
        self.processes: dict[int, list[str]] = {
            1: ["nightfall", "RR"],
            DEFAULT_SH_PID: ["sh[server]", "RR"],
            3: ["sleep", "S"],
            4: ["old", "Z"],
        }
        self.slow_to_exit: set[int] = set()
        self.exiting: set[int] = set()
        self.scripts: dict[str, Any] = {}
        self.commands: dict[str, Any] = {}
        self.files: dict[str, Any] = {}
        self.log_records: list[Any] = []
        self.calls: list[tuple] = []
        self.refusal: BaseException | None = None
        self.connections: list[FakeConnection] = []

    def connect(self, sh_server_pid: int | None) -> FakeConnection:
        pid = DEFAULT_SH_PID if sh_server_pid is None else sh_server_pid
        self.calls.append(("connect", pid))
        if self.refusal is not None:
            raise self.refusal
        if pid not in self.processes or self.processes[pid][1] == "Z":
            self.processes[pid] = ["sh[server]", "RR"]
        connection = FakeConnection(self, pid)
        self.connections.append(connection)
        return connection

    def shells(self) -> list[int]:
        return [call[1] for call in self.calls if call[0] == "connect"]

    def commands_in(self, pid: int) -> list[str]:
        return [call[2] for call in self.calls if call[0] == "sh" and call[1] == pid]

    def key_name(self, name: str) -> str:
        from dawn.facts import key_id

        return name if name in self.registered else f"{key_id(name):#018x}"

    def compile(self, words: list[str]) -> None:
        if words[0] in self.commands:
            return
        if words[:2] == ["kvs", "get"] and not self.matching(words[2]):
            raise RuntimeError(
                f"program args builder failed: no key matches `{words[2]}`"
            )
        if words[0] not in ("kvs", "ps", "kill", "cp", "echo", "logs"):
            raise RuntimeError(f"no sh entry found for `{words[0]}`")

    def matching(self, key: str) -> list[str]:
        return sorted(
            name
            for name in self.kvs
            if key == "*"
            or name == key
            or (name in self.registered and name.startswith(key))
        )

    def statement(self, connection: FakeConnection, words: list[str]) -> list:
        program = words[0]
        if program in self.commands:
            produced = self.commands[program]
            if callable(produced):
                answered: Any = produced(connection, words)
                return list(answered)
            return list(produced)
        if words[:2] == ["kvs", "get"]:
            return self.kvs_get(words[2])
        if words[:2] == ["kvs", "set"]:
            self.kvs[words[2]] = words[3]
            return []
        if words == ["ps"]:
            return self.ps()
        if program == "kill":
            return self.kill(words[1:])
        if program == "cp":
            return self.cp(words[1], words[2])
        if words[:2] == ["logs", "dump"]:
            return list(self.log_records)
        return [" ".join(words[1:])]

    def kvs_get(self, key: str) -> list:
        names = self.matching(key)
        return [
            {
                KVS_TYPE_ID: {
                    "Key": [self.key_name(name) for name in names],
                    "Value": [self.kvs[name] for name in names],
                }
            }
        ]

    def ps(self) -> list:
        for pid in self.exiting:
            if pid in self.processes:
                self.processes[pid][1] = "Z"
        self.exiting = set()
        pids = list(self.processes)
        return [
            {
                PS_TYPE_ID: {
                    "Name": [self.processes[pid][0] for pid in pids],
                    "Version": ["0.1.0" for _ in pids],
                    "Program ID": [0 for _ in pids],
                    "PID": pids,
                    "State": [self.processes[pid][1] for pid in pids],
                }
            }
        ]

    def kill(self, arguments: list[str]) -> list:
        signal = 15
        if arguments[0] == "--signal":
            signal, arguments = int(arguments[1]), arguments[2:]
        pid = int(arguments[0], 0)
        if pid not in self.processes:
            raise RuntimeError("Failed: couldn't find process")
        if signal == 8:
            if self.processes[pid][1] == "Z":
                del self.processes[pid]
            return []
        if self.processes[pid][1] == "Z":
            raise RuntimeError("Failed: process has exited")
        if pid in self.slow_to_exit:
            self.exiting.add(pid)
        else:
            self.processes[pid][1] = "Z"
        return []

    def cp(self, source: str, destination: str) -> list:
        import hashlib

        path = source.removeprefix(":")
        content = self.files.get(path)
        if content is None:
            raise RuntimeError(f"Failed: couldn't open `{path}`")
        if isinstance(content, BaseException):
            raise content
        chunks = content if isinstance(content, list) else [content]
        target = pathlib.Path(destination)
        digest = hashlib.sha256()
        steps: list = [Step(lambda: target.write_bytes(b""))]
        for chunk in chunks:
            if isinstance(chunk, (Wait, BaseException)):
                steps.append(chunk)
                continue
            digest.update(chunk)

            def append(chunk=chunk) -> None:
                with target.open("ab") as file:
                    file.write(chunk)

            steps.append(Step(append))
        length = sum(len(chunk) for chunk in chunks if isinstance(chunk, bytes))
        steps.append(
            {
                CP_TYPE_ID: {
                    "source": path,
                    "destination": destination,
                    "length": length,
                    "resumed": 0,
                    "sha256": digest.hexdigest(),
                }
            }
        )
        return steps


class FakeFleet:
    def __init__(self) -> None:
        self.nodes: dict[str, FakeNode] = {}
        self.targets: list = []
        self.failure: BaseException | None = None
        self.delay = 0.0
        self.connections = 0

    def add(self, node: FakeNode, suffix: str = "fleet.dusk.example") -> FakeNode:
        self.nodes[f"{node.namespace_id}.{suffix}"] = node
        return node

    def __call__(self, target, sh_server_pid: int | None):
        import time

        self.targets.append(target)
        if self.delay:
            time.sleep(self.delay)
        if self.failure is not None:
            raise self.failure
        node = self.nodes.get(target.server_name)
        if node is None:
            raise RuntimeError(
                f"Disconnected: node {target.server_name} is not connected"
            )
        connection = node.connect(sh_server_pid)
        self.connections += 1
        return connection


@pytest.fixture
def fleet() -> FakeFleet:
    return FakeFleet()


def node_ref(**fields):
    from dawn.models import NodeRef

    values = {
        "device_id": DEVICE_ID,
        "installation_id": INSTALLATION_ID,
        "namespace_id": NAMESPACE_ID,
        "nightfall": None,
    }
    values.update(fields)
    return NodeRef(**values)


def work_spec(**fields):
    from dawn.models import Work

    values = {
        "pid": str(PID),
        "campaign_id": CAMPAIGN_ID,
        "attempt": 1,
        "kind": "run_script",
        "script": "echo hello",
        "timeout_seconds": 30,
    }
    values.update(fields)
    return Work(**values)


SETTINGS = {
    "instance": "dawn-0",
    "tls": {
        "certificate": "/etc/dawn/tls/server.crt",
        "key": "/etc/dawn/tls/server.key",
    },
    "auth": {"tokens_file": "/etc/dawn/secrets/tokens.toml"},
    "kafka": {"allow_plaintext": True},
    "nightfall": {
        "default_inner_address": "nightfall-inner:8444",
        "allowed_inner_addresses": ["nightfall-*.nightfall-inner.dusk.svc:8444"],
    },
}


def dawn_settings(**overrides):
    from dawn.config import Settings

    values: dict = {
        key: dict(value) if isinstance(value, dict) else value
        for key, value in SETTINGS.items()
    }
    for key, value in overrides.items():
        if isinstance(value, dict) and isinstance(values.get(key), dict):
            values[key].update(value)
        else:
            values[key] = value
    return Settings(**values)


CONTRACTS = pathlib.Path(__file__).resolve().parents[2] / "contracts" / "kafka"
TOPICS = ("dusk.process-results", "dusk.process-output", "dusk.files")


def contract_validator():
    import json

    from jsonschema import Draft202012Validator
    from referencing import Registry, Resource

    registry = Registry().with_resources(
        (path.name, Resource.from_contents(json.loads(path.read_text())))
        for path in CONTRACTS.glob("*.schema.json")
    )
    validators = {
        topic: Draft202012Validator(
            json.loads((CONTRACTS / f"{topic}.schema.json").read_text()),
            registry=registry,
            format_checker=Draft202012Validator.FORMAT_CHECKER,
        )
        for topic in TOPICS
    }

    def validate(topic: str, message: dict) -> None:
        validators[topic].validate(json.loads(json.dumps(message)))

    return validate


class RecordingProducer:
    def __init__(self) -> None:
        self.sent: list[tuple[str, str, dict]] = []
        self.validate = contract_validator()

    async def send(self, topic: str, key: str, value: dict):
        self.validate(topic, value)
        self.sent.append((topic, key, value))

        async def delivered() -> bool:
            return True

        return delivered()

    def on(self, topic: str) -> list[dict]:
        return [value for sent_topic, _, value in self.sent if sent_topic == topic]

    def results(self) -> list[dict]:
        return self.on("dusk.process-results")


@pytest.fixture
def producer() -> RecordingProducer:
    return RecordingProducer()


DISPATCHER_TOKEN = "dispatcher-token"
OPERATOR_TOKEN = "operator-token"
SECOND_OPERATOR_TOKEN = "second-operator-token"
VIEWER_TOKEN = "viewer-token"
OPERATOR = "operator@example.org"
PROGRAMS = [
    {
        "name": "ps",
        "version": "0.1.0",
        "short_description": "List the processes on the node.",
        "long_description": "Usage: ps",
        "program_id": 15065078153151533341,
    }
]


class MemoryStorage:
    def __init__(self) -> None:
        self.objects: dict[str, bytes] = {}

    async def put_object(
        self, Bucket: str, Key: str, Body: bytes, ContentType: str
    ) -> dict:
        self.objects[Key] = Body
        return {}


def bearer(token: str) -> dict[str, str]:
    return {"authorization": f"Bearer {token}"}


class Dawn:
    def __init__(
        self, directory: pathlib.Path, connector=None, **settings_overrides
    ) -> None:
        import hashlib

        from dawn.api import Services, build_application
        from dawn.auth import Authenticator, TokenFile
        from dawn.config import KafkaTopics
        from dawn.dispatch import Dispatcher
        from dawn.events import Events
        from dawn.files import Files
        from dawn.logstreams import LogStreams
        from dawn.metrics import Metrics
        from dawn.nodes import Sessions
        from dawn.work import Results, Runner

        tokens = directory / "tokens.toml"
        entries = [
            (DISPATCHER_TOKEN, "dispatcher", "twilight-0"),
            (OPERATOR_TOKEN, "operator", OPERATOR),
            (SECOND_OPERATOR_TOKEN, "operator", "second@example.org"),
            (VIEWER_TOKEN, "viewer", "viewer@example.org"),
        ]
        tokens.write_text(
            "\n".join(
                f'[[token]]\nsha256 = "{hashlib.sha256(token.encode()).hexdigest()}"\n'
                f'role = "{role}"\nsubject = "{subject}"\n'
                for token, role, subject in entries
            )
        )
        overrides: dict = {"auth": {"tokens_file": str(tokens)}}
        overrides.update(settings_overrides)
        self.settings = dawn_settings(**overrides)
        self.fleet = FakeFleet()
        self.connector = connector or self.fleet
        self.producer = RecordingProducer()
        self.storage = MemoryStorage()
        self.metrics = Metrics()
        self.sessions = Sessions(
            self.settings.limits.max_node_sessions, self.metrics.node_sessions.set
        )
        self.results = Results(
            Events(self.producer, KafkaTopics(), self.settings.instance),
            b"k" * 32,
            self.metrics.counted,
        )
        limits = self.settings.limits
        self.files = Files(
            self.storage,
            self.settings.s3.bucket,
            self.results,
            limits.max_file_bytes,
            limits.max_concurrent_uploads,
            limits.max_staged_bytes,
        )
        runner = Runner(self.settings, self.connector, self.results, self.files)
        self.services = Services(
            settings=self.settings,
            authenticator=Authenticator(
                TokenFile(tokens), None, self.settings.principals
            ),
            connector=self.connector,
            sessions=self.sessions,
            results=self.results,
            dispatcher=Dispatcher(self.settings, runner, self.sessions),
            files=self.files,
            log_streams=LogStreams(
                self.settings, self.connector, self.sessions, self.results
            ),
            metrics=self.metrics,
            programs=PROGRAMS,
        )
        self.application = build_application(self.services, "0.0.0.0")


@pytest.fixture
def dawn(tmp_path):
    return Dawn(tmp_path)


@pytest.fixture
def client(dawn):
    from starlette.testclient import TestClient

    with TestClient(
        dawn.application, base_url="https://dawn-0.dawn:8443"
    ) as test_client:
        yield test_client
