from __future__ import annotations

import os
import pathlib
import shutil
import signal
import socket
import subprocess
import tempfile
import time

import pytest

REPOSITORY = pathlib.Path(__file__).resolve().parents[3]
NODE_BINARY = REPOSITORY / "target/debug/kvs_persistent_node"
CLI_BINARY = REPOSITORY / "target/debug/dusk"
CLIENTS = pathlib.Path(__file__).resolve().parent / "clients"
ADDRESS = "127.0.0.1"
DOCKER_LIMITS = ["--cpu-shares", "2"] + (
    ["--cpuset-cpus", os.environ["DUSK_TEST_CPUSET"]]
    if os.environ.get("DUSK_TEST_CPUSET")
    else []
)


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind((ADDRESS, 0))
        return probe.getsockname()[1]


def wait_for(port: int, process: subprocess.Popen, timeout: float = 30.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise AssertionError(
                f"exited with {process.returncode} before {ADDRESS}:{port} came up"
            )
        try:
            with socket.create_connection((ADDRESS, port), timeout=0.2):
                return
        except OSError:
            time.sleep(0.05)
    raise AssertionError(f"nothing came up on {ADDRESS}:{port}")


class Node:
    def __init__(self, file: pathlib.Path) -> None:
        self.file = file
        self.port = free_port()
        self.process = subprocess.Popen(
            [str(NODE_BINARY), self.address, str(file)],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        wait_for(self.port, self.process)
        subprocess.run(
            [str(CLI_BINARY), self.address, "logs dump --replay-only"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=30,
            check=True,
        )

    @property
    def address(self) -> str:
        return f"{ADDRESS}:{self.port}"

    def run(self, command: str) -> str:
        finished = subprocess.run(
            [str(CLI_BINARY), self.address, command],
            capture_output=True,
            text=True,
            timeout=30,
            env={**os.environ, "DUSK_NON_INTERACTIVE": "1"},
        )
        return finished.stdout + finished.stderr

    def crash(self) -> None:
        self.process.kill()
        self.process.wait(timeout=10)


class Binding:
    def __init__(self, node: Node, *options: str) -> None:
        self.port = free_port()
        self.output = tempfile.TemporaryFile(mode="w+")
        self.process = subprocess.Popen(
            [
                str(CLI_BINARY),
                node.address,
                " ".join(["kvs bind", *options, self.address]),
            ],
            stdin=subprocess.DEVNULL,
            stdout=self.output,
            stderr=subprocess.STDOUT,
        )
        try:
            wait_for(self.port, self.process)
        except AssertionError as error:
            raise AssertionError(f"{error}:\n{self.read()}") from None

    @property
    def address(self) -> str:
        return f"{ADDRESS}:{self.port}"

    def read(self) -> str:
        self.output.seek(0)
        return self.output.read()

    def stop(self) -> str:
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGINT)
        try:
            self.process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        return self.read()


def docker(image: str, mounts: list[str], command: str) -> subprocess.CompletedProcess:
    if shutil.which("docker") is None:
        pytest.skip("this client runs in Docker, and docker is not on PATH")
    return subprocess.run(
        [
            "docker",
            "run",
            "--rm",
            "--network",
            "host",
            *DOCKER_LIMITS,
            "--user",
            f"{os.getuid()}:{os.getgid()}",
            "-e",
            "HOME=/tmp",
            *[argument for mount in mounts for argument in ("-v", mount)],
            image,
            "sh",
            "-c",
            command,
        ],
        capture_output=True,
        text=True,
        timeout=900,
    )


@pytest.fixture(scope="session", autouse=True)
def binaries():
    if not NODE_BINARY.exists() or not CLI_BINARY.exists():
        pytest.skip(
            "build the binaries first: cargo build --bin dusk --bin kvs_persistent_node"
        )


@pytest.fixture(scope="session")
def caches(tmp_path_factory) -> pathlib.Path:
    return tmp_path_factory.mktemp("client-caches")


@pytest.fixture
def node(tmp_path):
    node = Node(tmp_path / "kvs")
    yield node
    node.crash()


@pytest.fixture
def binding(node):
    binding = Binding(node)
    yield binding
    binding.stop()
