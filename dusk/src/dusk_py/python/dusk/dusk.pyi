"""Type stub for the native ``dusk`` extension (compiled from ``dusk_py``).

Mirrors the PyO3 surface defined in ``dusk/src/dusk_py``. Keep in sync with the
``#[pymethods]`` blocks on ``Dusk`` and ``ShellOutput`` there.
"""

from typing import Any

class ShellOutput:
    """A shell command's output objects (unpickled Values), as they arrive.

    Read it either way. Iterating blocks the calling thread until the next value
    arrives; awaiting :meth:`next_value` does not, which is what lets one thread
    read many commands at once.
    """

    def __iter__(self) -> ShellOutput: ...
    def __next__(self) -> Any: ...
    async def next_value(self) -> Any:
        """The next value. Raises ``StopAsyncIteration`` once the command ends."""
        ...

class Dusk:
    """Client connection to a Dusk server."""

    def __init__(self, address: str, port: int) -> None: ...
    def disconnect(self) -> None: ...
    def sh(self, command: str) -> ShellOutput: ...
    @staticmethod
    def help(program_name: str = ...) -> Any: ...
