import importlib.util
import json
import shutil
from pathlib import Path

import pytest
from jsonschema import Draft202012Validator

specification = importlib.util.spec_from_file_location(
    "validate", Path(__file__).parents[1] / "validate.py"
)
assert specification is not None and specification.loader is not None
validate = importlib.util.module_from_spec(specification)
specification.loader.exec_module(validate)


@pytest.fixture
def kafka(tmp_path: Path) -> Path:
    return Path(shutil.copytree(validate.KAFKA, tmp_path / "kafka"))


def rewrite(path: Path, **changes: object) -> None:
    message = json.loads(path.read_text())
    message.update(changes)
    path.write_text(json.dumps(message))


def test_shipped_contracts_hold() -> None:
    assert validate.failures(validate.KAFKA) == []


def test_main_exits_zero_and_prints_nothing(capsys: pytest.CaptureFixture[str]) -> None:
    assert validate.main() == 0
    assert capsys.readouterr().out == ""


def test_every_topic_has_a_schema_and_examples() -> None:
    topics = {
        "dusk.connections",
        "dusk.census",
        "dusk.ledger",
        "dusk.enrollments",
        "dusk.node-state",
        "dusk.process-results",
        "dusk.process-output",
        "dusk.files",
        "dusk.intended-processes",
    }
    schemas = {
        path.name.removesuffix(".schema.json")
        for path in validate.KAFKA.glob("*.schema.json")
    }
    assert schemas == topics | {"common"}
    for topic in topics:
        examples = list((validate.KAFKA / "examples" / topic).glob("*.json"))
        valid = [path for path in examples if not path.name.startswith("invalid-")]
        invalid = [path for path in examples if path.name.startswith("invalid-")]
        assert len(valid) >= 2, topic
        assert len(invalid) >= 4, topic


def test_relative_reference_resolves_to_common() -> None:
    registry = validate.registry(validate.KAFKA)
    resolved = registry.resolver().lookup("common.schema.json#/$defs/device_id")
    assert resolved.contents == {"type": "string", "pattern": "^[0-9a-f]{32}$"}


def test_pid_is_exactly_a_u64_in_decimal() -> None:
    validator = Draft202012Validator(
        {"$ref": "common.schema.json#/$defs/pid"},
        registry=validate.registry(validate.KAFKA),
    )
    maximum = 2**64 - 1
    digits = str(maximum)
    numbers = {0, 1, 9, 10, 10**19 - 1, 10**19, 10**20 - 1, maximum - 1, maximum}
    numbers.add(maximum + 1)
    for position in range(len(digits)):
        for digit in "0123456789":
            numbers.add(int(digits[:position] + digit + digits[position + 1 :]))
    for number in numbers:
        assert validator.is_valid(str(number)) == (number <= maximum), number
    for pid in ["", "00", "01", "-1", "+1", "1.0", "1e3", " 1", "0x1f", "1" + "0" * 20]:
        assert not validator.is_valid(pid), pid
    assert not validator.is_valid(1)


def test_valid_example_that_fails_is_reported(kafka: Path) -> None:
    example = kafka / "examples" / "dusk.connections" / "connected.json"
    rewrite(example, device_id="NOT-HEX")
    assert validate.failures(kafka) == [
        "examples/dusk.connections/connected.json: $.device_id: "
        "'NOT-HEX' does not match '^[0-9a-f]{32}$'"
    ]


def test_every_failure_is_reported(kafka: Path) -> None:
    rewrite(kafka / "examples" / "dusk.files" / "campaign-file.json", size_bytes=-1)
    rewrite(kafka / "examples" / "dusk.ledger" / "checkpoint.json", signature=None)
    found = validate.failures(kafka)
    assert len(found) == 2
    assert found[0].startswith("examples/dusk.files/campaign-file.json: $.size_bytes")
    assert found[1].startswith("examples/dusk.ledger/checkpoint.json: $.signature")


def test_invalid_example_that_passes_is_reported(kafka: Path) -> None:
    directory = kafka / "examples" / "dusk.node-state"
    shutil.copy(
        directory / "device-revoked.json", directory / "invalid-extra-property.json"
    )
    assert validate.failures(kafka) == [
        "examples/dusk.node-state/invalid-extra-property.json: "
        "passed, but an invalid example must fail"
    ]


def test_invalid_example_with_two_defects_is_reported(kafka: Path) -> None:
    example = kafka / "examples" / "dusk.node-state" / "invalid-reason-type.json"
    rewrite(example, actor="")
    found = validate.failures(kafka)
    assert len(found) == 1
    assert found[0].startswith(
        "examples/dusk.node-state/invalid-reason-type.json: failed 2 ways"
    )


def test_enum_value_without_valid_example_is_reported(kafka: Path) -> None:
    (kafka / "examples" / "dusk.connections" / "disconnected-killed.json").unlink()
    assert validate.failures(kafka) == [
        'dusk.connections: no valid example has disconnect_reason = "killed"'
    ]


def test_null_enum_value_without_valid_example_is_reported(kafka: Path) -> None:
    directory = kafka / "examples" / "dusk.connections"
    for path in directory.glob("connected*.json"):
        path.unlink()
    found = validate.failures(kafka)
    assert "dusk.connections: no valid example has disconnect_reason = null" in found
    assert 'dusk.connections: no valid example has event = "connected"' in found


def test_examples_without_schema_are_reported(kafka: Path) -> None:
    (kafka / "dusk.files.schema.json").unlink()
    assert validate.failures(kafka) == [
        "examples/dusk.files: no dusk.files.schema.json beside it"
    ]


def test_topic_without_examples_is_reported(kafka: Path) -> None:
    shutil.rmtree(kafka / "examples" / "dusk.process-output")
    assert validate.failures(kafka) == ["dusk.process-output: no examples"]


def test_malformed_example_is_reported(kafka: Path) -> None:
    (kafka / "examples" / "dusk.files" / "campaign-file.json").write_text("{")
    found = validate.failures(kafka)
    assert len(found) == 1
    assert found[0].startswith(
        "examples/dusk.files/campaign-file.json: could not be validated"
    )


def test_schema_that_is_not_draft_2020_12_is_reported(kafka: Path) -> None:
    schema_path = kafka / "dusk.process-output.schema.json"
    schema = json.loads(schema_path.read_text())
    schema["properties"]["index"]["minimum"] = "zero"
    schema_path.write_text(json.dumps(schema))
    found = validate.failures(kafka)
    assert found[0].startswith(
        "dusk.process-output.schema.json: not a valid draft 2020-12 schema"
    )


def test_unresolvable_reference_is_reported(kafka: Path) -> None:
    (kafka / "common.schema.json").rename(kafka / "shared.json")
    unvalidated = {
        failure.split(":")[0]
        for failure in validate.failures(kafka)
        if "could not be validated" in failure
    }
    valid = {
        f"examples/{path.parent.name}/{path.name}"
        for path in kafka.glob("examples/*/*.json")
        if not path.name.startswith("invalid-")
    }
    assert valid <= unvalidated


def test_main_exits_non_zero_and_prints_every_failure(
    kafka: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    rewrite(kafka / "examples" / "dusk.files" / "campaign-file.json", size_bytes=-1)
    rewrite(kafka / "examples" / "dusk.ledger" / "checkpoint.json", signature=None)
    assert validate.main(kafka) == 1
    assert len(capsys.readouterr().out.splitlines()) == 2
