import re
import shlex
from pathlib import Path

from harness import (
    WATERMARKS,
    agent_type,
    ask,
    decisions_path,
    deny,
    drive_state,
    lint_branch,
    lint_message,
    read_input,
    reviewed,
    session_state,
)

READ_ONLY_AGENTS = {"self-review", "race-screen", "race-inspector", "dilemma-triage", "decision-ranker"}
REDIRECT = re.compile(r"\d*>>?(.*)")
MUTATING = re.compile(
    r"\bgit\s+(commit|push|reset|rebase|checkout|switch|stash|add|rm|mv|tag|cherry-pick|merge|am|apply)\b"
    r"|\b(rm|mv|cp|tee)\b|\bsed\s+-i\b"
)


INTERPRETER = re.compile(r"(ba|z|k|da)?sh|python3?|perl|ruby|node|deno|awk|xargs|env")


def interpreter(word):
    return bool(INTERPRETER.fullmatch(word.rsplit("/", 1)[-1]))


def fed_to_interpreter(line):
    segments = line.split("|")
    for index, segment in enumerate(segments):
        if "<<" not in segment:
            continue
        for candidate in [segment] + segments[index + 1:]:
            words = candidate.split()
            if words and interpreter(words[0]):
                return True
        return False
    return False


def writes_into(command, cwd):
    try:
        tokens = shlex.split(command, comments=False)
    except ValueError:
        return True
    root = Path(cwd).resolve()
    for index, token in enumerate(tokens):
        redirect = REDIRECT.fullmatch(token)
        if not redirect:
            continue
        name = redirect.group(1) or (tokens[index + 1] if index + 1 < len(tokens) else "")
        if not name or name.startswith("&"):
            continue
        target = (root / name).resolve()
        if target == root or root in target.parents:
            return True
    return False


def without_heredocs(command):
    def strip(match):
        start = command.rfind("\n", 0, match.start()) + 1
        end = command.find("\n", match.start())
        if fed_to_interpreter(command[start:end if end != -1 else len(command)]):
            return match.group(0)
        return match.group(1)

    return re.sub(r"(<<-?\s*'?(\w+)'?\s*\n).*?\n\2\b", strip, command, flags=re.S)


def commit_messages(command):
    messages = []
    for match in re.finditer(r"""(?:^|\s)(?:-m|--message)(?:=|\s+)("((?:[^"\\]|\\.)*)"|'([^']*)'|(\S+))""", command):
        messages.append(match.group(2) or match.group(3) or match.group(4) or "")
    heredoc = re.search(r"<<-?\s*'?(\w+)'?\s*\n(.*?)\n\1\b", command, re.S)
    if heredoc:
        messages.append(heredoc.group(2))
    return ["\n\n".join(messages)] if messages else []


def gh_body(command, cwd):
    file_match = re.search(r"--body-file[= ]+(\S+)|-F\s+body=@(\S+)", command)
    if file_match:
        name = (file_match.group(1) or file_match.group(2)).strip("'\"")
        try:
            return Path(name).read_text() if name.startswith("/") else (Path(cwd) / name).read_text()
        except OSError:
            return ""
    body_match = re.search(r"""(?:--body|-b)[= ]+("((?:[^"\\]|\\.)*)"|'([^']*)')""", command, re.S)
    if body_match:
        return body_match.group(2) or body_match.group(3) or ""
    return ""


def pull_request_problems(cwd, body, creating, hook_input):
    problems = []
    if any(mark in body for mark in WATERMARKS):
        problems.append("the PR body carries a watermark; never")
    branch = reviewed(cwd, hook_input)
    _, drive = drive_state(branch, cwd)
    _, session = session_state(hook_input.get("session_id", ""))
    issue = drive.get("issue") or (session.get("drive") or {}).get("issue")
    if issue and not re.search(rf"(Closes|Part of) #{issue}\b", body):
        problems.append(
            f"the PR body must carry `Closes #{issue}`, or `Part of #{issue}` "
            "when it is one of a stack and another pull request closes the issue"
        )
    if not {"harness", "drive-issue"} & set(session["loaded_skills"]):
        return problems
    if creating and drive.get("pr"):
        problems.append(f"one pull request per drive: update #{drive['pr']} instead of opening another")
    path = decisions_path(cwd, branch)
    if not path.exists():
        problems.append(f"{path.relative_to(cwd)} does not exist: every decision goes through the dilemma-triage agent as it is made")
    elif "## Decisions" not in body:
        problems.append(f"the PR body must carry the decisions from {path.relative_to(cwd)} under `## Decisions`")
    return problems


def main():
    hook_input = read_input()
    tool_input = hook_input.get("tool_input", {})
    command = tool_input.get("command", "")
    cwd = hook_input.get("cwd", ".")
    agent = agent_type(hook_input)
    code = without_heredocs(command)

    def has(pattern):
        return re.search(pattern, code) is not None

    if agent in READ_ONLY_AGENTS and (MUTATING.search(code) or writes_into(code, cwd)):
        deny(
            f"The {agent} agent reads; it does not change the tree or the history. Write what you "
            "need to keep into a path outside the working directory."
        )
    if has(r"\b(until|while)\b[^\n]*\bsleep\b") or has(r"\bsleep\s+\d+[^\n]*\b(grep|tail|cat|test|ls)\b"):
        deny("Never poll for a command you started: the harness re-invokes you when a background command exits (dusk-developer, Never poll for a command I started).")
    if has(r"\bcp\b[^\n;&|]*\btarget\b"):
        deny("Never copy a target directory; park it with mv and move it back (drive-issue, Step 2).")
    if has(r"\bgit\s+stash\b(?!\s+(push|list|show|apply|drop|branch)\b)"):
        deny("No bare git stash and no stash pop: the stash stack is shared with every worktree. Use git stash push -u -m <tag>, restore with git stash apply <sha>, then drop it by tag.")
    if has(r"\bcargo\s+(test|nextest)\b"):
        ask("Tests run only when the user asked for them (dusk-developer, Don't write or run tests unless told). Did they ask?")
    snapshot = has(r"\bgit\s+commit\b") and any("snapshot" in message.lower() for message in commit_messages(command))
    if has(r"\bgit\s+add\b[^\n;&|]*(\s-A\b|\s--all\b|\s\.(?=\s|$))") and not snapshot:
        deny("Stage named paths, never git add -A or git add . - only the atomic-commit snapshot is taken that way.")
    if has(r"\bgit\s+commit\b"):
        timeout = tool_input.get("timeout") or 120000
        verifies = not has(r"--no-verify\b")
        if tool_input.get("run_in_background") or (verifies and timeout < 600000):
            deny("Run git commit in the foreground, and with timeout 600000 when it runs the hooks: pre-commit stashes unstaged work, and a commit killed mid-hook strands it (dusk-developer, Never poll for a command I started).")
        for message in commit_messages(command):
            problems = lint_message(message)
            if problems:
                deny("Commit message: " + "; ".join(problems))
    if has(r"\bgit\s+push\b"):
        if has(r"\bgit\s+push\b[^\n;&|]*(\s|:)(master|main)(\s|$)"):
            deny("Never push to master; open a pull request.")
        problems = lint_branch(cwd)
        if problems:
            deny("Fix these commits before pushing: " + "; ".join(problems))
    if has(r"\bgh\s+pr\s+create\b"):
        problems = pull_request_problems(cwd, gh_body(command, cwd), True, hook_input)
        if problems:
            deny("Pull request: " + "; ".join(problems))
    if has(r"\bgh\s+pr\s+merge\b") or (has(r"\bgh\s+api\b") and has(r"/merge\b")):
        ask("Merging is the one gate that may not be skipped: did the user approve this merge, explicitly, for this pull request?")


if __name__ == "__main__":
    main()
