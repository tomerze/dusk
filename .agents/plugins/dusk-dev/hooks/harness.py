import contextlib
import fcntl
import hashlib
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
READY_LINE = "Ready for review."
DILEMMA_LINE = "DILEMMA:"
WAITING_LINE = "WAITING ON SUBAGENTS:"
BACKGROUND_LINE = "WAITING ON BACKGROUND WORK:"
EXITED = "[exited with code"
REPORT_HEADINGS = (
    "WHAT IS OVERALL STATE:",
    "WHAT I DID NOW:",
    "WHAT DO I NEED FROM YOU:",
)
STANDING_LINE = {
    "ready": "Still ready for review.",
    "dilemma": "Still waiting for dilemma verdict.",
}


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


def amend(path, change):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with holding(path):
        current = read_json(path, {})
        change(current)
        replace_text(path, json.dumps(current, indent=2, sort_keys=True) + "\n")
        AS_LOADED[str(path)] = deepcopy(current)
        return current


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


def branch_name(branch):
    return (branch or "detached").removeprefix("worktree-")


def session_state(session_id):
    path = data_dir() / "sessions" / f"{safe_name(session_id)}.json"
    default = {"loaded_skills": [], "drive": None, "driver_checks": {}, "stop_blocks": {}, "activate_nudged": False, "committed": False, "standing": None, "handed": None, "agents": [], "background": []}
    state = load_json(path, default)
    for key, value in default.items():
        state.setdefault(key, value)
    AS_LOADED[str(path)] = deepcopy(state)
    return path, state


def running_background(session):
    live = []
    for job in session.get("background") or []:
        output = job.get("output") if isinstance(job, dict) else None
        if not output:
            continue
        try:
            text = Path(output).read_text()
        except OSError:
            continue
        if EXITED not in text:
            live.append(job.get("id") or Path(output).stem)
    return live


def drive_state(branch, cwd="."):
    where = repository(cwd)
    key = safe_name(branch_name(branch))
    if where:
        key = hashlib.sha256(where.encode()).hexdigest()[:12] + "-" + key
    path = data_dir() / "drives" / f"{key}.json"
    return path, load_json(path, {})


def git(cwd, *args):
    try:
        completed = subprocess.run(
            ["git", *args], cwd=cwd, capture_output=True, text=True, check=False
        )
    except OSError:
        return 1, ""
    return completed.returncode, completed.stdout.strip()


def current_branch(cwd):
    code, branch = git(cwd, "rev-parse", "--abbrev-ref", "HEAD")
    return branch if code == 0 else None


def reviewed(cwd, hook_input):
    _, session = session_state(hook_input.get("session_id", ""))
    return session.get("handed") or current_branch(cwd)


def agent_type(hook_input):
    return (hook_input.get("agent_type") or "").split(":")[-1]


def review_dir(cwd, branch):
    return Path(cwd) / "review" / safe_name(branch_name(branch))


def decisions_path(cwd, branch):
    return review_dir(cwd, branch) / "decisions.md"


LOCATION = re.compile(r"(?<![\w/])((?:[\w.-]+/)*[\w.-]+\.[A-Za-z]\w*):(\d+)")


def findings(message):
    return [
        line.rstrip()
        for line in (message or "").splitlines()
        if line.startswith(("- ", "|")) and LOCATION.search(line)
    ]


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
        _, files = git(cwd, "show", "--name-only", "--format=", short)
        paths = files.split()
        if paths and all(path.startswith("docs/") for path in paths) and not subject.startswith("Update docs"):
            problems.append(f"{short}: `{subject}` changes only docs/; a docs commit's subject starts `Update docs`")
        if paths and all(path.startswith("tests/") for path in paths) and not subject.startswith("Add tests"):
            problems.append(f"{short}: `{subject}` changes only tests/; a tests commit's subject starts `Add tests`")
    return problems


READY_SIGNOFFS = (("split_sha", "the atomic-commit split"),)
REVIEW_SIGNOFFS = (
    ("reviewed_sha", "self-review"),
    ("race_sha", "the race screen"),
    ("terminology_sha", "terminology-review"),
    ("strings_sha", "string-review"),
    ("ranked_sha", "decision-ranker"),
)


def tip(cwd, branch=None):
    code, sha = git(cwd, "rev-parse", branch or "HEAD")
    return sha if code == 0 else None


def missing_signoffs(cwd, signoffs=READY_SIGNOFFS, branch=None):
    branch = branch or current_branch(cwd)
    at = tip(cwd, branch)
    _, drive = drive_state(branch, cwd)
    return [name for key, name in signoffs if drive.get(key) != at]


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


def headless_agent(agent, prompt, cwd, timeout=180):
    plugin_root = os.environ.get("CLAUDE_PLUGIN_ROOT") or str(Path(__file__).resolve().parent.parent)
    try:
        completed = subprocess.run(
            [
                "claude", "-p", "--plugin-dir", plugin_root, "--agent", f"{PLUGIN}:{agent}",
                "--output-format", "text", "--permission-mode", "bypassPermissions", prompt,
            ],
            cwd=cwd,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    return completed.stdout.strip() if completed.returncode == 0 else None


def pull_request_view(cwd, branch):
    try:
        completed = subprocess.run(
            ["gh", "pr", "view", branch, "--json", "number,state,headRefOid,body,url"],
            cwd=cwd,
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if completed.returncode != 0:
        return None
    try:
        return json.loads(completed.stdout)
    except ValueError:
        return None


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
