import os
import re
from pathlib import Path

from harness import deny, haiku, read_input, save_json, session_state

COMMENT_LINE = {
    ".rs": r"^\s*//",
    ".c": r"^\s*//",
    ".h": r"^\s*//",
    ".capnp": r"^\s*#",
    ".py": r"^\s*#(?!!)",
    ".toml": r"^\s*#",
}
PROGRAM_SUFFIXES = {".rs", ".capnp", ".toml"}
DRIVER_QUESTION = (
    "You are checking one edit to the Dusk codebase. Dusk has a `Driver` trait in "
    "dusk_core that each platform impl implements (hostname, exit, launchers). "
    "Does this edit add, remove, rename or change the signature of a method of "
    "that Driver trait, or of the extern shim that carries it? Answer with the "
    "single word yes or no.\n\nFILE: {path}\n\nBEFORE:\n{old}\n\nAFTER:\n{new}\n"
)


def relative(path, cwd):
    if not path:
        return ""
    return os.path.relpath(path, cwd) if os.path.isabs(path) else path


def main():
    hook_input = read_input()
    tool_input = hook_input.get("tool_input", {})
    cwd = hook_input.get("cwd", ".")
    path = tool_input.get("file_path", "")
    suffix = Path(path).suffix
    if hook_input.get("tool_name") == "Edit":
        old, new = tool_input.get("old_string", ""), tool_input.get("new_string", "")
    else:
        old, new = "", tool_input.get("content", "")

    pattern = COMMENT_LINE.get(suffix)
    if pattern:
        old_lines = set(old.splitlines())
        added = [line for line in new.splitlines() if re.match(pattern, line) and line not in old_lines]
        if added:
            deny(
                "I do not write comments (AGENTS.md). Say it in the reply and let the user decide "
                f"whether it becomes one: {added[0].strip()!r}"
            )

    rel = relative(path, cwd)
    session_path, session = session_state(hook_input.get("session_id", ""))
    loaded = set(session["loaded_skills"])
    if rel.startswith("base/") and suffix in PROGRAM_SUFFIXES and "authoring-a-program" not in loaded:
        deny(
            "This is a Dusk program. Invoke /dusk-dev:authoring-a-program first — it covers a "
            "program's schema, args, launcher, process, portal and shell entry — then make the edit."
        )
    driver_paths = rel.startswith("impls/") or rel.startswith("dusk/src/dusk_core/src/driver")
    if driver_paths and suffix == ".rs" and "adding-a-driver-method" not in loaded:
        verdict = session["driver_checks"].get(rel)
        if verdict is None:
            answer = haiku(DRIVER_QUESTION.format(path=rel, old=old[:3000], new=new[:3000]))
            verdict = bool(answer) and answer.strip().lower().startswith("yes")
            session["driver_checks"][rel] = verdict
            save_json(session_path, session)
        if verdict:
            deny(
                "This changes the Driver trait. Invoke /dusk-dev:adding-a-driver-method first — "
                "a Driver method is added in four layers, modelled on hostname — then make the edit."
            )


if __name__ == "__main__":
    main()
