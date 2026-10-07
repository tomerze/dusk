import json
from pathlib import Path

import pytest
from bundle_schemas import bundle
from jsonschema import Draft202012Validator
from referencing import Registry, Resource

CONTRACTS = Path(__file__).resolve().parents[2] / "services" / "contracts" / "kafka"


def original_validator(path: Path) -> Draft202012Validator:
    registry = Registry().with_resources(
        (schema.name, Resource.from_contents(json.loads(schema.read_text())))
        for schema in CONTRACTS.glob("*.schema.json")
    )
    return Draft202012Validator(json.loads(path.read_text()), registry=registry)


def examples() -> list[tuple[str, Path]]:
    return [
        (directory.name, example)
        for directory in sorted((CONTRACTS / "examples").iterdir())
        for example in sorted(directory.glob("*.json"))
    ]


@pytest.fixture(scope="module")
def bundled(tmp_path_factory: pytest.TempPathFactory) -> Path:
    output = tmp_path_factory.mktemp("bundled")
    bundle(CONTRACTS, output)
    return output


def test_every_topic_schema_is_bundled_without_external_references(
    bundled: Path,
) -> None:
    names = sorted(path.name for path in bundled.glob("*.schema.json"))
    expected = sorted(
        path.name
        for path in CONTRACTS.glob("*.schema.json")
        if path.name != "common.schema.json"
    )
    assert names == expected
    for path in bundled.glob("*.schema.json"):
        assert "common.schema.json" not in path.read_text()
        Draft202012Validator.check_schema(json.loads(path.read_text()))


@pytest.mark.parametrize(
    ("topic", "example"), examples(), ids=lambda value: getattr(value, "name", value)
)
def test_a_bundled_schema_gives_every_example_the_verdict_of_the_original(
    bundled: Path, topic: str, example: Path
) -> None:
    message = json.loads(example.read_text())
    original = sorted(
        error.message
        for error in original_validator(CONTRACTS / f"{topic}.schema.json").iter_errors(
            message
        )
    )
    standalone = Draft202012Validator(
        json.loads((bundled / f"{topic}.schema.json").read_text())
    )
    assert (
        sorted(error.message for error in standalone.iter_errors(message)) == original
    )
    assert bool(original) == example.name.startswith("invalid-")


def test_refuses_a_reference_it_cannot_inline(tmp_path: Path) -> None:
    contracts = tmp_path / "contracts"
    contracts.mkdir()
    (contracts / "common.schema.json").write_text(json.dumps({"$defs": {}}))
    (contracts / "dusk.example.schema.json").write_text(
        json.dumps({"$ref": "https://example.com/other.json"})
    )
    with pytest.raises(ValueError, match="cannot bundle the reference"):
        bundle(contracts, tmp_path / "output")


def test_refuses_a_definition_that_would_shadow_a_shared_one(tmp_path: Path) -> None:
    contracts = tmp_path / "contracts"
    contracts.mkdir()
    (contracts / "common.schema.json").write_text(
        json.dumps({"$defs": {"id": {"type": "string"}}})
    )
    (contracts / "dusk.example.schema.json").write_text(
        json.dumps({"$defs": {"common.id": {}}})
    )
    with pytest.raises(ValueError, match="already defines common.id"):
        bundle(contracts, tmp_path / "output")
