"""Type stub for the native ``dusk`` extension (compiled from ``dusk_py``).

Mirrors the PyO3 surface defined in ``dusk/src/dusk_py``. Keep in sync with the
``#[pymethods]`` blocks on ``Dusk`` and ``ShellOutput`` there.
"""

from typing import Any

class ShellOutput:
    """Iterator over a shell command's output objects (unpickled Values)."""

    def __iter__(self) -> ShellOutput: ...
    def __next__(self) -> Any: ...

class Dusk:
    """Client connection to a Dusk server."""

    def __init__(self, address: str, port: int) -> None: ...
    def disconnect(self) -> None: ...
    def sh(self, command: str) -> ShellOutput: ...
    @staticmethod
    def help(program_name: str = ...) -> Any: ...
