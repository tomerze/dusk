from __future__ import annotations

import resource

import pytest

from dawn import file_limit


class Limits:
    def __init__(self, soft: int, hard: int) -> None:
        self.current = (soft, hard)
        self.set_to: tuple[int, int] | None = None

    def get(self, which: int) -> tuple[int, int]:
        assert which == resource.RLIMIT_NOFILE
        return self.current

    def set(self, which: int, limits: tuple[int, int]) -> None:
        assert which == resource.RLIMIT_NOFILE
        self.set_to = limits
        self.current = limits


@pytest.fixture
def limits(monkeypatch: pytest.MonkeyPatch):
    def install(soft: int, hard: int) -> Limits:
        fake = Limits(soft, hard)
        monkeypatch.setattr(file_limit.resource, "getrlimit", fake.get)
        monkeypatch.setattr(file_limit.resource, "setrlimit", fake.set)
        return fake

    return install


def test_the_soft_limit_is_raised_to_the_hard_limit(limits):
    fake = limits(1024, 1048576)

    assert file_limit.raise_file_limit(10000) == 1048576
    assert fake.set_to == (1048576, 1048576)


def test_dawn_refuses_to_start_when_the_hard_limit_cannot_hold_its_sessions(limits):
    fake = limits(1024, 20000)

    with pytest.raises(file_limit.FileLimitTooLow, match="21024"):
        file_limit.raise_file_limit(10000)
    assert fake.set_to is None


def test_a_limit_already_at_the_hard_limit_is_left_alone(limits):
    fake = limits(65536, 65536)

    assert file_limit.raise_file_limit(10000) == 65536
    assert fake.set_to is None


def test_an_unlimited_hard_limit_raises_the_soft_limit_to_what_is_needed(limits):
    fake = limits(1024, resource.RLIM_INFINITY)

    assert file_limit.raise_file_limit(10000) == 21024
    assert fake.set_to == (21024, resource.RLIM_INFINITY)
