import json
import sys
from collections.abc import Iterator
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator
from jsonschema.exceptions import SchemaError, ValidationError
from referencing import Registry, Resource

KAFKA = Path(__file__).parent / "kafka"
SUFFIX = ".schema.json"
COMMON = "common" + SUFFIX


def load(path: Path) -> Any:
    return json.loads(path.read_text())


def registry(directory: Path) -> Registry:
    return Registry().with_resources(
        (path.name, Resource.from_contents(load(path)))
        for path in directory.glob("*" + SUFFIX)
    )


def describe(error: ValidationError) -> str:
    if not error.context:
        return f"{error.json_path}: {error.message}"
    branches: dict[str | int, list[ValidationError]] = {}
    for child in error.context:
        branches.setdefault(child.relative_schema_path[0], []).append(child)
    closest = min(
        branches.values(),
        key=lambda children: (
            all(child.validator_value == "null" for child in children),
            len(children),
        ),
    )
    return "; ".join(describe(child) for child in closest)


def enumerated(schema: Any) -> Iterator[tuple[str, str]]:
    if isinstance(schema, list):
        for item in schema:
            yield from enumerated(item)
    elif isinstance(schema, dict):
        for name, subschema in schema.get("properties", {}).items():
            if isinstance(subschema, dict):
                for value in subschema.get("enum", []):
                    yield name, json.dumps(value)
        for child in schema.values():
            yield from enumerated(child)


def present(message: Any) -> Iterator[tuple[str, str]]:
    if isinstance(message, list):
        for item in message:
            yield from present(item)
    elif isinstance(message, dict):
        for name, value in message.items():
            yield name, json.dumps(value)
            yield from present(value)


def failures(directory: Path) -> list[str]:
    schemas = registry(directory)
    found: list[str] = []
    topics = sorted(
        path.name.removesuffix(SUFFIX)
        for path in directory.glob("*" + SUFFIX)
        if path.name != COMMON
    )
    examples_directory = directory / "examples"
    for stray in sorted(
        path.name for path in examples_directory.iterdir() if path.name not in topics
    ):
        found.append(f"examples/{stray}: no {stray}{SUFFIX} beside it")
    for name in sorted(path.name for path in directory.glob("*" + SUFFIX)):
        try:
            Draft202012Validator.check_schema(schemas[name].contents)
        except SchemaError as failure:
            found.append(f"{name}: not a valid draft 2020-12 schema: {failure}")
    for topic in topics:
        schema = schemas[topic + SUFFIX].contents
        validator = Draft202012Validator(schema, registry=schemas)
        examples = sorted((examples_directory / topic).glob("*.json"))
        if not examples:
            found.append(f"{topic}: no examples")
        covered: set[tuple[str, str]] = set()
        for path in examples:
            label = f"examples/{topic}/{path.name}"
            try:
                message = load(path)
                errors = list(validator.iter_errors(message))
            except Exception as failure:
                found.append(f"{label}: could not be validated: {failure}")
                continue
            if not path.name.startswith("invalid-"):
                covered.update(present(message))
                found.extend(f"{label}: {describe(error)}" for error in errors)
            elif not errors:
                found.append(f"{label}: passed, but an invalid example must fail")
            elif len(errors) > 1:
                found.append(
                    f"{label}: failed {len(errors)} ways, but an invalid example "
                    "holds exactly one defect: "
                    + "; ".join(describe(error) for error in errors)
                )
        for name, value in sorted(set(enumerated(schema)) - covered):
            found.append(f"{topic}: no valid example has {name} = {value}")
    return found


def main(directory: Path = KAFKA) -> int:
    found = failures(directory)
    for failure in found:
        print(failure)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
