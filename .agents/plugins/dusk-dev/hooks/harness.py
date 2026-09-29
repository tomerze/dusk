import contextlib
import fcntl
import json
import os
import re
import subprocess
import sys
from copy import deepcopy
from pathlib import Path

PLUGIN = "dusk-dev"
HAIKU = "claude-haiku-4-5-20251001"
WATERMARKS = ("Co-Authored-By", "Claude-Session", "Generated with")
CONVENTIONAL_PREFIX = re.compile(
    r"^(feat|fix|chore|docs|refactor|test|style|perf|build|ci|revert)(\([^)]*\))?!?:", re.I
)
ISSUE_IN_SUBJECT = re.compile(r"\(#\d+\)")


def read_input():
    raw = sys.stdin.read()
    return json.loads(raw) if raw.strip() else {}


def data_dir():
    root = os.environ.get("DUSK_HARNESS_DATA") or str(
        Path.home() / ".claude" / "plugins" / "data" / PLUGIN
    )
    path = Path(root)
    path.mkdir(parents=True, exist_ok=True)
    return path


AS_LOADED = {}


@contextlib.contextmanager
def holding(path):
    lock = Path(str(path) + ".lock")
    lock.parent.mkdir(parents=True, exist_ok=True)
    with lock.open("w") as handle:
        fcntl.flock(handle, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(handle, fcntl.LOCK_UN)


def replace_text(path, text):
    temporary = Path(str(path) + ".new")
    temporary.write_text(text)
    os.replace(temporary, path)


def read_json(path, default):
    try:
        return json.loads(Path(path).read_text())
    except (OSError, ValueError):
        return default


def load_json(path, default):
    value = read_json(path, default)
    AS_LOADED[str(path)] = deepcopy(value)
    return value


def save_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with holding(path):
        current = read_json(path, {})
        before = AS_LOADED.get(str(path), {})
        if isinstance(current, dict) and isinstance(value, dict):
            for key, now in value.items():
                if key not in current or key not in before or before[key] != now:
                    current[key] = now
            for key in before:
                if key not in value and current.get(key) == before[key]:
                    current.pop(key, None)
        else:
            current = value
        replace_text(path, json.dumps(current, indent=2, sort_keys=True) + "\n")
        AS_LOADED[str(path)] = deepcopy(value)


def safe_name(name):
    return re.sub(r"[^A-Za-z0-9._-]", "_", name or "detached")


def personal(cwd):
    strings = {Path.home().name, str(Path.home())}
    for setting in ("user.name", "user.email"):
        code, value = git(cwd, "config", "--get", setting)
        if code == 0 and value:
            strings.add(value)
            strings.update(value.split("@")[0].split())
    return {value for value in strings if len(value) > 2}


def repository(cwd):
    code, common = git(cwd, "rev-parse", "--path-format=absolute", "--git-common-dir")
    return common if code == 0 else ""


def session_state(session_id):
    path = data_dir() / "sessions" / f"{safe_name(session_id)}.json"
    default = {"loaded_skills": [], "driver_checks": {}, "activate_nudged": False}
    state = load_json(path, default)
    for key, value in default.items():
        state.setdefault(key, value)
    AS_LOADED[str(path)] = deepcopy(state)
    return path, state


def git(cwd, *args):
    try:
        completed = subprocess.run(
            ["git", *args], cwd=cwd, capture_output=True, text=True, check=False
        )
    except OSError:
        return 1, ""
    return completed.returncode, completed.stdout.strip()


def agent_type(hook_input):
    return (hook_input.get("agent_type") or "").split(":")[-1]


def lint_message(message):
    lines = message.strip().splitlines()
    if not lines:
        return ["empty message"]
    subject = lines[0].strip()
    problems = []
    if CONVENTIONAL_PREFIX.match(subject):
        problems.append(f"`{subject}` has a conventional-commit prefix; a subject is one imperative sentence")
    if re.match(r"^Say\b", subject):
        problems.append(f"`{subject}` starts with Say; name what was done to the file")
    if ISSUE_IN_SUBJECT.search(subject):
        problems.append(f"`{subject}` carries an issue number; that goes in the PR body as Closes #N")
    for line in lines:
        if any(mark in line for mark in WATERMARKS):
            problems.append(f"watermark trailer `{line.strip()}`; never")
            break
    return problems


def lint_branch(cwd, base="origin/master"):
    code, log = git(cwd, "log", "--format=%h%x00%B%x01", f"{base}..HEAD")
    if code != 0:
        return []
    problems = []
    for entry in filter(None, log.split("\x01")):
        short, _, message = entry.strip("\n").partition("\x00")
        subject = message.strip().splitlines()[0] if message.strip() else ""
        if re.match(r"^WIP\b", subject, re.I):
            problems.append(f"{short} is still a WIP commit; split it with atomic-commit first")
        problems.extend(f"{short}: {problem}" for problem in lint_message(message))
        listed, files = git(cwd, "show", "--name-only", "--format=", short)
        if listed != 0:
            problems.append(f"{short}: could not list its changed paths; the docs/tests marker was not checked")
            continue
        paths = files.splitlines()
        if paths and all(path.startswith("docs/") for path in paths) and not subject.startswith("Update docs"):
            problems.append(f"{short}: `{subject}` changes only docs/; a docs commit's subject starts `Update docs`")
        if paths and all(path.startswith("tests/") for path in paths) and not subject.startswith(("Add tests", "Update tests")):
            problems.append(f"{short}: `{subject}` changes only tests/; a tests commit's subject starts `Add tests`, or `Update tests` when it changes tests that exist")
    return problems


def haiku(prompt, timeout=60):
    try:
        completed = subprocess.run(
            ["claude", "-p", "--model", HAIKU, "--output-format", "text", prompt],
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    return completed.stdout.strip() if completed.returncode == 0 else None


def deny(reason):
    print(reason, file=sys.stderr)
    sys.exit(2)


def ask(reason):
    print(
        json.dumps(
            {
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "ask",
                    "permissionDecisionReason": reason,
                }
            }
        )
    )
    sys.exit(0)


def context(text):
    print(text)
    sys.exit(0)
