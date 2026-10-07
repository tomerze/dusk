import json
import sys
from pathlib import Path
from typing import Any

COMMON = "common.schema.json"
EXTERNAL = COMMON + "#/$defs/"
LOCAL = "#/$defs/"
PREFIX = "common."


def rewrite(node: Any, references: dict[str, str]) -> Any:
    if isinstance(node, list):
        return [rewrite(item, references) for item in node]
    if not isinstance(node, dict):
        return node
    rewritten = {key: rewrite(value, references) for key, value in node.items()}
    reference = rewritten.get("$ref")
    if isinstance(reference, str):
        for source, target in references.items():
            if reference.startswith(source):
                rewritten["$ref"] = target + reference.removeprefix(source)
                break
        else:
            if not reference.startswith(LOCAL):
                raise ValueError(f"cannot bundle the reference {reference}")
    return rewritten


def bundle(directory: Path, output: Path) -> list[Path]:
    common = json.loads((directory / COMMON).read_text())
    shared = rewrite(common.get("$defs", {}), {LOCAL: LOCAL + PREFIX})
    output.mkdir(parents=True, exist_ok=True)
    written = []
    for path in sorted(directory.glob("*.schema.json")):
        if path.name == COMMON:
            continue
        schema = rewrite(json.loads(path.read_text()), {EXTERNAL: LOCAL + PREFIX})
        definitions = schema.setdefault("$defs", {})
        for name, definition in shared.items():
            if PREFIX + name in definitions:
                raise ValueError(f"{path.name} already defines {PREFIX + name}")
            definitions[PREFIX + name] = definition
        target = output / path.name
        target.write_text(json.dumps(schema, indent=2) + "\n")
        written.append(target)
    return written


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit(
            "usage: bundle_schemas.py <contracts kafka directory> <output directory>"
        )
    for path in bundle(Path(sys.argv[1]), Path(sys.argv[2])):
        print(f"bundled {path}")


if __name__ == "__main__":
    main()
