"""Fixtures for the interactive prompt tests.

The prompt is a full-screen TUI: it takes over a terminal, asks that terminal
where the cursor is, and repaints on every keystroke. Testing it therefore needs
a terminal, not a pipe - these tests give it one with ``ttyd``, which puts a real
pty behind a websocket, and drive that websocket the way a person drives a
keyboard. A bare pty is not enough: nothing in it answers the cursor-position
query, and the prompt gives up.

Each test gets its own node (a real ``dusk_node`` process, which always listens
on port 9090, so the tests run one at a time) and its own terminal, so tests
never share shell state.
"""

from __future__ import annotations

import asyncio
import json
import os
import pathlib
import re
import shutil
import socket
import subprocess
import time

import pytest

ANSI = re.compile(r"\x1b\[[0-9;?]*[a-zA-Z]|\x1b\][^\x07]*\x07|\x1b[=>()][A-Za-z0-9]?|[\x07\x08]")


def readable(raw: str) -> str:
    """The text a person would see, with the terminal's control codes gone."""
    return ANSI.sub("", raw)

REPOSITORY = pathlib.Path(__file__).resolve().parents[2]
NODE_BINARY = REPOSITORY / "target/debug/dusk_node"
CLI_BINARY = REPOSITORY / "target/debug/dusk"
ADDRESS = "127.0.0.1"
NODE_PORT = 9090


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind((ADDRESS, 0))
        return probe.getsockname()[1]


def wait_for(port: int, timeout: float = 10.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with socket.create_connection((ADDRESS, port), timeout=0.2):
                return
        except OSError:
            time.sleep(0.05)
    raise AssertionError(f"nothing came up on {ADDRESS}:{port}")


class Node:
    """A dusk_node of this test's own, plus the client commands to poke it."""

    def __init__(self) -> None:
        self.port = NODE_PORT
        with socket.socket() as probe:
            if probe.connect_ex((ADDRESS, self.port)) == 0:
                raise AssertionError(f"something already listens on {ADDRESS}:{self.port}")
        self.process = subprocess.Popen(
            [str(NODE_BINARY)],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        wait_for(self.port)

    @property
    def address(self) -> str:
        return f"{ADDRESS}:{self.port}"

    def run(self, command: str, timeout: float = 20.0) -> str:
        """Run one shell command through a non-interactive client."""
        finished = subprocess.run(
            [str(CLI_BINARY), self.address, command],
            capture_output=True,
            text=True,
            timeout=timeout,
            env={**os.environ, "DUSK_NON_INTERACTIVE": "1"},
        )
        return finished.stdout + finished.stderr

    def processes(self) -> dict:
        """The ps table as columns, e.g. `processes()["Name"]`."""
        output = self.run("ps")
        decoder = json.JSONDecoder()
        for line in output.splitlines():
            line = line.strip()
            if not line.startswith("{"):
                continue
            table, _ = decoder.raw_decode(line)
            return next(iter(table.values()))
        raise AssertionError(f"no ps table in:\n{output}")

    def named(self, name: str) -> list[int]:
        table = self.processes()
        return [
            pid
            for pid, process in zip(table["PID"], table["Name"])
            if process.startswith(name)
        ]

    def alive(self) -> bool:
        return '"' in self.run("hostname")

    def stop(self) -> None:
        self.process.terminate()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()


class Terminal:
    """A prompt on a real terminal, driven a keystroke at a time."""

    def __init__(self, command: list[str], environment: dict | None = None) -> None:
        self.port = free_port()
        self.process = subprocess.Popen(
            ["ttyd", "-p", str(self.port), "-W", "-o", *command],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            env={**os.environ, **(environment or {})},
        )
        wait_for(self.port)
        self.loop = asyncio.new_event_loop()
        self.screen: list[str] = []
        self.socket = self.loop.run_until_complete(self._connect())

    async def _connect(self):
        import websockets

        socket = await websockets.connect(
            f"ws://{ADDRESS}:{self.port}/ws", subprotocols=["tty"], open_timeout=10
        )
        await socket.send(
            json.dumps({"AuthToken": "", "columns": 110, "rows": 30}).encode()
        )
        return socket

    async def _read_for(self, seconds: float) -> None:
        end = asyncio.get_event_loop().time() + seconds
        while True:
            remaining = end - asyncio.get_event_loop().time()
            if remaining <= 0:
                return
            try:
                message = await asyncio.wait_for(self.socket.recv(), timeout=remaining)
            except (asyncio.TimeoutError, Exception):
                return
            if isinstance(message, bytes) and message[:1] == b"0":
                output = message[1:]
                if b"\x1b[6n" in output:
                    # A terminal answers where the cursor is; so must we.
                    await self.socket.send(b"0\x1b[1;1R")
                self.screen.append(output.decode("utf-8", "replace"))

    def read(self, seconds: float = 2.0) -> str:
        self.loop.run_until_complete(self._read_for(seconds))
        return readable("".join(self.screen))

    def type(self, line: str, settle: float = 2.5, until: str | None = None) -> str:
        # Type at an idle prompt, the way a person does - and give reedline the
        # beat it needs to be reading again before the keystrokes land.
        self.wait_for("❯", timeout=10)
        time.sleep(0.4)
        self.read(0.2)
        self.since()
        self.loop.run_until_complete(self.socket.send(b"0" + line.encode()))
        self.read(0.3)
        self.loop.run_until_complete(self.socket.send(b"0\r"))
        if until is not None:
            return self.wait_for(until)
        return self.read(settle)

    def press(self, key: bytes, settle: float = 2.5) -> str:
        self.loop.run_until_complete(self.socket.send(b"0" + key))
        return self.read(settle)

    def wait_for(self, marker: str, timeout: float = 15.0) -> str:
        """Read until `marker` shows up, or give up after `timeout`."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            shown = self.read(0.5)
            if marker in shown:
                return shown
        return readable("".join(self.screen))

    def gone(self, timeout: float = 15.0) -> bool:
        """True once the program under this terminal has exited."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if not self.running():
                return True
            time.sleep(0.25)
        return not self.running()

    def since(self) -> str:
        """Everything shown so far, then start a fresh page."""
        shown = readable("".join(self.screen))
        self.screen.clear()
        return shown

    def running(self) -> bool:
        return self.process.poll() is None

    def stop(self) -> None:
        try:
            self.loop.run_until_complete(self.socket.close())
        except Exception:
            pass
        self.loop.close()
        self.process.terminate()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()


@pytest.fixture(scope="session", autouse=True)
def binaries():
    if not NODE_BINARY.exists() or not CLI_BINARY.exists():
        pytest.skip("build the binaries first: cargo build --bin dusk --bin dusk_node")
    if shutil.which("ttyd") is None:
        pytest.skip("these tests drive a real terminal and need ttyd on PATH")


@pytest.fixture
def node():
    node = Node()
    yield node
    node.stop()


@pytest.fixture
def prompt(node):
    """A `dusk <address>` prompt, open and drawn."""
    terminal = Terminal([str(CLI_BINARY), node.address])
    assert "❯" in terminal.wait_for("❯"), "the prompt never drew itself"
    terminal.since()
    yield terminal
    terminal.stop()
