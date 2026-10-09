import fnmatch
import os
import re
import tomllib
from pathlib import Path
from typing import Any

import pytest

REPOSITORY = Path(
    os.environ.get("REPOSITORY_DIRECTORY") or Path(__file__).resolve().parents[2]
)
PERMISSIONS = REPOSITORY / "infra" / "k8s" / "base" / "nightfall" / "permissions.toml"
SCHEMAS = [
    *sorted(REPOSITORY.glob("dusk/src/dusk_capnp/capnp/*.capnp")),
    *sorted(REPOSITORY.glob("base/*/capnp/*.capnp")),
]
DECLARATION = re.compile(
    r"\binterface\s+(?P<interface>\w+)|\bstruct\s+(?P<struct>\w+)"
    r"|(?P<method>\w+)\s+@\d+\s*\(|(?P<brace>[{}])"
)

DAWN_CALLS_ON_THE_NODE = [
    "Dusk.process",
    "Dusk.run",
    "Dusk.ps",
    "Dusk.kill",
    "Dusk.waitpid",
    "Dusk.programs",
    "Process.pid",
    "Process.portal",
    "Portal.programId",
    "OutputPortal.output",
    "ShPortal.sh",
    "ShPortal.functions",
    "SignalBatch.Ack.ack",
]
THE_NODE_CALLS_ON_DAWN = [
    "Stream.send",
    "Stream.done",
    "Created.created",
    "ShStop.stop",
    "LogsArgs.Server.openStream",
    "LogsArgs.Stream.send",
    "LogsArgs.Stream.stop",
    "KvsArgs.Server.transpose",
    "ProgramsArgs.Server.transpose",
    "CpArgs.Server.stat",
    "CpArgs.Server.hash",
    "CpArgs.Server.write",
    "Sink.write",
    "Sink.done",
]


def schema_methods() -> dict[str, set[str]]:
    interfaces: dict[str, set[str]] = {}
    for path in SCHEMAS:
        text = re.sub(r"#[^\n]*", "", path.read_text())
        scope: list[str | None] = []
        pending: str | None = None
        for match in DECLARATION.finditer(text):
            if match["interface"] or match["struct"]:
                pending = match["interface"] or match["struct"]
                if match["interface"]:
                    name = ".".join([*filter(None, scope), match["interface"]])
                    interfaces.setdefault(name, set())
            elif match["method"]:
                interfaces[".".join(filter(None, scope))].add(match["method"])
            elif match["brace"] == "{":
                scope.append(pending)
                pending = None
            else:
                scope.pop()
    return interfaces


def split(pattern: str) -> tuple[str, str]:
    interface, _, method = pattern.rpartition(".")
    return interface, method


def matches(pattern: str, call: str) -> bool:
    pattern_interface, pattern_method = split(pattern)
    interface, method = split(call)
    return fnmatch.fnmatchcase(interface, pattern_interface) and fnmatch.fnmatchcase(
        method, pattern_method
    )


def roles_of(principal: str) -> list[dict[str, Any]]:
    permissions = tomllib.loads(PERMISSIONS.read_text())
    names = {
        role
        for entry in permissions["principal"]
        if fnmatch.fnmatchcase(principal, entry["name"])
        for role in entry["roles"]
    }
    return [role for role in permissions["role"] if role["name"] in names]


def test_the_schemas_were_found() -> None:
    methods = schema_methods()
    assert methods["ShStop"] == {"stop"}
    assert methods["LogsArgs.Server"] == {"openStream"}
    assert {"stat", "hash", "read", "write"} == methods["CpArgs.Server"]


@pytest.mark.parametrize(
    "role",
    tomllib.loads(PERMISSIONS.read_text())["role"],
    ids=lambda role: role["name"],
)
def test_every_pattern_names_a_method_in_the_schemas(role: dict[str, Any]) -> None:
    calls = [
        f"{interface}.{method}"
        for interface, methods in schema_methods().items()
        for method in methods
    ]
    for key in ["allow", "deny", "reverse_allow"]:
        for pattern in role.get(key, []):
            assert any(matches(pattern, call) for call in calls), (
                f"{role['name']}.{key}: {pattern} matches no method"
            )


@pytest.mark.parametrize("call", DAWN_CALLS_ON_THE_NODE)
def test_dawn_may_make_every_call_its_flows_make_on_the_node(call: str) -> None:
    roles = roles_of("dawn-0")
    assert any(
        matches(pattern, call) for role in roles for pattern in role.get("allow", [])
    )
    assert not any(
        matches(pattern, call) for role in roles for pattern in role.get("deny", [])
    )


@pytest.mark.parametrize("call", THE_NODE_CALLS_ON_DAWN)
def test_the_node_may_make_every_call_back_into_dawn(call: str) -> None:
    assert any(
        matches(pattern, call)
        for role in roles_of("dawn-0")
        for pattern in role.get("reverse_allow", [])
    )


@pytest.mark.parametrize("call", ["Dusk.settime", "Dusk.fleetToken"])
def test_dawn_is_denied(call: str) -> None:
    roles = roles_of("dawn-0")
    assert any(
        matches(pattern, call) for role in roles for pattern in role.get("deny", [])
    )


def test_only_the_break_glass_role_is_exempt_from_admission_and_no_principal_holds_it() -> None:
    permissions = tomllib.loads(PERMISSIONS.read_text())
    exempt = [
        role["name"]
        for role in permissions["role"]
        if role.get("admission_exempt", False)
    ]
    assert exempt == ["break-glass"]
    assert not any("break-glass" in entry["roles"] for entry in permissions["principal"])
    assert not any(role.get("admission_exempt", False) for role in roles_of("dawn-0"))


def test_quarantine_allows_only_reading_who_the_node_is() -> None:
    permissions = tomllib.loads(PERMISSIONS.read_text())
    quarantine = next(
        role for role in permissions["role"] if role["name"] == "quarantine"
    )
    assert sorted(quarantine["allow"]) == [
        "Dusk.hostname",
        "Dusk.namespaceId",
        "Dusk.programs",
        "Dusk.time",
    ]
    assert quarantine["reverse_allow"] == []
