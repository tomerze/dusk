from __future__ import annotations

import importlib.util
import pathlib
import sys
import types

import pytest

PYTHON_SOURCE = pathlib.Path(__file__).resolve().parents[3] / "dusk/src/dusk_py/python"

if importlib.util.find_spec("dusk") is None:
    sys.path.insert(0, str(PYTHON_SOURCE))
    sys.modules["dusk.dusk"] = types.ModuleType("dusk.dusk")


@pytest.fixture
def anyio_backend() -> str:
    return "asyncio"
