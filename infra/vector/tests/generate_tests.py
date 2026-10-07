import json
import sys
from pathlib import Path
from typing import Any

TABLES = {
    "dusk.ledger": "ledger",
    "dusk.connections": "connections",
    "dusk.enrollments": "enrollments",
    "dusk.process-results": "process_results",
    "dusk.process-output": "process_output",
    "dusk.files": "files",
}
PARTITION = 2
OFFSET = 4096


def vrl_string(text: str) -> str:
    encoded = json.dumps(text)
    if "\\" in encoded:
        raise ValueError(
            f"{encoded} needs an escape, which this generator does not translate to VRL"
        )
    return encoded


def raw_json(message: Any) -> str:
    encoded = json.dumps(message, separators=(",", ":"), ensure_ascii=False)
    if "'" in encoded:
        raise ValueError(
            "a fixture holds a single quote, which a VRL raw string cannot carry"
        )
    return f"s'{encoded}'"


def columns(schema: Path) -> list[str]:
    names = []
    for line in schema.read_text().splitlines():
        if line.startswith("  ") and not line.startswith("   ") and line.strip() != "}":
            names.append(line.split()[2].rstrip(";"))
    return names


def test_case(topic: str, table: str, example: Path, lake: Path) -> dict[str, Any]:
    message = json.loads(example.read_text())
    insert = (
        f'. = {{"message": {raw_json(message)}, "topic": "{topic}", '
        f'"partition": {PARTITION}, "offset": {OFFSET}}}'
    )
    case: dict[str, Any] = {
        "name": f"{table} {example.stem}",
        "inputs": [{"insert_at": table, "type": "vrl", "source": insert}],
    }
    if example.name.startswith("invalid-"):
        case["no_outputs_from"] = [table]
        return case
    expected = sorted([*columns(lake / f"{table}.schema"), "dt"])
    assertions = [
        f"assert_eq!(keys(.), {json.dumps(expected)})",
        f"assert_eq!(.id, {vrl_string(message['id'])})",
        f"assert_eq!(.schema, {vrl_string(message['schema'])})",
        f"assert_eq!(.kafka_partition, {PARTITION})",
        f"assert_eq!(.kafka_offset, {OFFSET})",
        f"assert_eq!(.time, t'{message['time']}')",
        f"assert_eq!(.dt, {vrl_string(message['time'][:10])})",
    ]
    case["outputs"] = [
        {
            "extract_from": table,
            "conditions": [{"type": "vrl", "source": "\n".join(assertions)}],
        }
    ]
    return case


def generate(contracts: Path, lake: Path, output: Path) -> list[Path]:
    output.mkdir(parents=True, exist_ok=True)
    written = []
    for topic, table in TABLES.items():
        examples = sorted((contracts / "examples" / topic).glob("*.json"))
        if not examples:
            raise ValueError(f"no examples for {topic}")
        tests = [test_case(topic, table, example, lake) for example in examples]
        target = output / f"{table}.generated.json"
        target.write_text(json.dumps({"tests": tests}, indent=2) + "\n")
        written.append(target)
    return written


def main() -> None:
    if len(sys.argv) != 4:
        raise SystemExit(
            "usage: generate_tests.py <contracts kafka directory> <lake schema directory> <output directory>"
        )
    for path in generate(Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])):
        print(f"generated {path}")


if __name__ == "__main__":
    main()
