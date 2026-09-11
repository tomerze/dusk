import json
import os
import re
import subprocess
import sys
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
    root = os.environ.get("CLAUDE_PLUGIN_DATA") or str(
        Path.home() / ".claude" / "plugins" / "data" / PLUGIN
    )
    path = Path(root)
    path.mkdir(parents=True, exist_ok=True)
    return path


def load_json(path, default):
    try:
        return json.loads(Path(path).read_text())
    except (OSError, ValueError):
        return default


def save_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def safe_name(name):
    return re.sub(r"[^A-Za-z0-9._-]", "_", name or "detached")


def branch_name(branch):
    return (branch or "detached").removeprefix("worktree-")


def session_state(session_id):
    path = data_dir() / "sessions" / f"{safe_name(session_id)}.json"
    default = {"loaded_skills": [], "drive": None, "driver_checks": {}, "stop_blocks": {}}
    state = load_json(path, default)
    for key, value in default.items():
        state.setdefault(key, value)
    return path, state


def drive_state(branch):
    path = data_dir() / "drives" / f"{safe_name(branch_name(branch))}.json"
    return path, load_json(path, {})


def git(cwd, *args):
    completed = subprocess.run(
        ["git", *args], cwd=cwd, capture_output=True, text=True, check=False
    )
    return completed.returncode, completed.stdout.strip()


def current_branch(cwd):
    code, branch = git(cwd, "rev-parse", "--abbrev-ref", "HEAD")
    return branch if code == 0 else None


def head(cwd):
    code, sha = git(cwd, "rev-parse", "HEAD")
    return sha if code == 0 else None


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
    return problems


READY_SIGNOFFS = (("split_sha", "the atomic-commit split"),)
REVIEW_SIGNOFFS = (
    ("reviewed_sha", "self-review"),
    ("race_sha", "the race screen"),
    ("terminology_sha", "terminology-review"),
    ("strings_sha", "string-review"),
    ("ranked_sha", "decision-ranker"),
)


def missing_signoffs(cwd, signoffs=READY_SIGNOFFS):
    branch = current_branch(cwd)
    tip = head(cwd)
    _, drive = drive_state(branch)
    return [name for key, name in signoffs if drive.get(key) != tip]


def field(message, name):
    matches = re.findall(rf"^[*_ \t]*{name}:[ \t]*(.*?)[*_ \t]*$", message or "", re.M)
    return matches[-1].strip() if matches else None


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


def block(reason):
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


def allow_with_input(updated_input):
    print(
        json.dumps(
            {
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "allow",
                    "updatedInput": updated_input,
                }
            }
        )
    )
    sys.exit(0)


def context(text):
    print(text)
    sys.exit(0)
