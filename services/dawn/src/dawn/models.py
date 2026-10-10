from __future__ import annotations

import posixpath
from typing import Annotated, Any, Literal

from pydantic import (
    AfterValidator,
    BaseModel,
    ConfigDict,
    Field,
    StringConstraints,
    model_validator,
)

MAX_PID = 2**64 - 1
DEFAULT_SH_PID = 0xF2EFCE60E8C425D0
RESERVED_BELOW = 2**16


def unreserved(pid: str) -> str:
    value = int(pid)
    if value > MAX_PID:
        raise ValueError(f"pid {pid} does not fit in 64 bits")
    if value < RESERVED_BELOW or value == DEFAULT_SH_PID:
        raise ValueError(f"pid {pid} is reserved")
    return pid


def pid_field(pid: int | str | None) -> str | None:
    return None if pid is None else f"{int(pid):x}"


DeviceId = Annotated[str, StringConstraints(pattern=r"^[0-9a-f]{32}$")]
InstallationId = Annotated[str, StringConstraints(pattern=r"^[0-9a-f]{32}$")]
NamespaceId = Annotated[str, StringConstraints(pattern=r"^[0-9a-f]{16}$")]
Pid = Annotated[
    str,
    StringConstraints(pattern=r"^[1-9][0-9]{4,19}$"),
    AfterValidator(unreserved),
]
CampaignId = Annotated[
    str,
    StringConstraints(
        pattern=r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
    ),
]
KvsKey = Annotated[
    str, StringConstraints(pattern=r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$")
]
NodePath = Annotated[str, StringConstraints(min_length=1, max_length=4096)]
Script = Annotated[str, StringConstraints(min_length=1, max_length=1048576)]
Endpoint = Annotated[
    str, StringConstraints(pattern=r"^(\[[0-9A-Fa-f:.]+\]|[A-Za-z0-9.-]+):[0-9]{1,5}$")
]
Level = Literal["error", "warn", "info", "debug", "trace"]
WorkKind = Literal["run_script", "ensure_version", "ensure_config", "quarantine"]

MAX_COLLECTED_FILES = 16
MAX_REAPED_PIDS = 256
DEVICE_ID_KEY = "dusk.device.id"


class StrictModel(BaseModel):
    model_config = ConfigDict(strict=True, extra="forbid")


class NodeRef(StrictModel):
    device_id: DeviceId
    installation_id: InstallationId
    namespace_id: NamespaceId
    nightfall: Endpoint | None = None


class LogStreamSpec(StrictModel):
    level: Level
    duration_seconds: int = Field(ge=1)


class Work(StrictModel):
    pid: Pid
    campaign_id: CampaignId | None = None
    attempt: int | None = Field(default=None, ge=1)
    kind: WorkKind
    script: Script | None = None
    timeout_seconds: int = Field(ge=1)
    version_key: KvsKey | None = None
    desired_version: str | None = Field(default=None, max_length=1024)
    config_hash: str | None = Field(default=None, max_length=1024)
    collect_facts: bool = False
    collect_files: list[NodePath] = Field(
        default_factory=list, max_length=MAX_COLLECTED_FILES
    )
    stream_logs: LogStreamSpec | None = None

    @model_validator(mode="after")
    def _fields_for_kind(self) -> Work:
        missing = [
            name for name in REQUIRED_BY_KIND[self.kind] if getattr(self, name) is None
        ]
        if missing:
            raise ValueError(f"{self.kind} work needs {', '.join(missing)}")
        if self.version_key == DEVICE_ID_KEY:
            raise ValueError(f"{DEVICE_ID_KEY} is never read as a version")
        for path in self.collect_files:
            check_path("collect_files entry", path)
        if len(set(self.collect_files)) != len(self.collect_files):
            raise ValueError("collect_files names a path twice")
        return self


REQUIRED_BY_KIND: dict[str, tuple[str, ...]] = {
    "run_script": ("script",),
    "ensure_version": ("script", "version_key", "desired_version"),
    "ensure_config": ("script", "config_hash"),
    "quarantine": ("script",),
}


def file_name(path: str) -> str:
    return posixpath.basename(path.replace("\\", "/"))


def quotable(word: str) -> bool:
    return "'" not in word or '"' not in word


def check_path(name: str, path: str) -> None:
    if not file_name(path):
        raise ValueError(f"{name} {path!r} names no file")
    if "\x00" in path:
        raise ValueError(f"{name} {path!r} holds a NUL character")
    if not quotable(path):
        raise ValueError(f"{name} {path!r} holds both quote characters")


class DispatchRequest(StrictModel):
    node: NodeRef
    work: list[Work] = Field(min_length=1)

    @model_validator(mode="after")
    def _one_work_per_pid(self) -> DispatchRequest:
        if len({work.pid for work in self.work}) != len(self.work):
            raise ValueError("a dispatch names a pid twice")
        return self


class ReapRequest(StrictModel):
    node: NodeRef
    pids: list[Pid] = Field(min_length=1, max_length=MAX_REAPED_PIDS)


class Accepted(BaseModel):
    accepted: list[str]


class FactsRequest(StrictModel):
    node: NodeRef
    pid: Pid
    version_keys: list[KvsKey] = Field(default_factory=list, max_length=16)

    @model_validator(mode="after")
    def _never_the_device_id(self) -> FactsRequest:
        if DEVICE_ID_KEY in self.version_keys:
            raise ValueError(f"{DEVICE_ID_KEY} is never read as a version")
        return self


class Reported(BaseModel):
    version_key: str | None
    version: str | None
    config_hash: str | None
    services: list[str] | None
    facts: dict[str, Any] | None


class FactsResponse(BaseModel):
    facts: dict[str, Any]
    reported: Reported


class LogStreamRequest(StrictModel):
    node: NodeRef
    pid: Pid
    level: Level
    duration_seconds: int = Field(ge=1)
    endpoint: Endpoint | None = None


class LogStreamStarted(BaseModel):
    stream_id: str


class LogStream(BaseModel):
    stream_id: str
    device_id: str
    installation_id: str
    namespace_id: str
    pid: str
    level: Level
    endpoint: str
    started_at: str
    ends_at: str
    principal: str


class FileRequest(StrictModel):
    node: NodeRef
    pid: Pid
    path: NodePath
    campaign_id: CampaignId | None = None

    @model_validator(mode="after")
    def _names_a_file(self) -> FileRequest:
        check_path("path", self.path)
        return self


class FileAccepted(BaseModel):
    upload_id: str


class ConnectRequest(StrictModel):
    node: NodeRef
    pid: Pid
