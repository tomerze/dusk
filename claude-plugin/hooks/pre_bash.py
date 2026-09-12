import re
from pathlib import Path

from harness import (
    WATERMARKS,
    agent_type,
    ask,
    current_branch,
    decisions_path,
    deny,
    drive_state,
    lint_branch,
    lint_message,
    missing_signoffs,
    read_input,
    session_state,
)

READ_ONLY_AGENTS = {"self-review", "race-screen", "race-inspector", "dilemma-triage", "decision-ranker"}
MUTATING = re.compile(
    r"\bgit\s+(commit|push|reset|rebase|checkout|switch|stash|add|rm|mv|tag|cherry-pick|merge|am|apply)\b"
    r"|\b(rm|mv|cp|tee)\b|\bsed\s+-i\b|>>?\s*\S"
)


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
    branch = current_branch(cwd)
    _, drive = drive_state(branch)
    _, session = session_state(hook_input.get("session_id", ""))
    issue = drive.get("issue") or (session.get("drive") or {}).get("issue")
    if issue and not re.search(rf"Closes #{issue}\b", body):
        problems.append(f"the PR body must carry `Closes #{issue}`")
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

    def has(pattern):
        return re.search(pattern, command) is not None

    if agent in READ_ONLY_AGENTS and MUTATING.search(command):
        deny(f"The {agent} agent reads; it does not change the tree or the history.")
    if has(r"\b(until|while)\b[^\n]*\bsleep\b") or has(r"\bsleep\s+\d+[^\n]*\b(grep|tail|cat|test|ls)\b"):
        deny("Never poll for a command you started: the harness re-invokes you when a background command exits (AGENTS.md, Never poll for a command I started).")
    if has(r"\bcp\b[^\n;&|]*\btarget\b"):
        deny("Never copy a target directory; park it with mv and move it back (drive-issue, Step 2).")
    if has(r"\bgit\s+stash\b(?!\s+(push|list|show|apply|drop|branch)\b)"):
        deny("No bare git stash and no stash pop: the stash stack is shared with every worktree. Use git stash push -u -m <tag>, restore with git stash apply <sha>, then drop it by tag.")
    if has(r"\bcargo\s+(test|nextest)\b"):
        ask("Tests run only when the user asked for them (AGENTS.md, Don't write or run tests unless told). Did they ask?")
    if has(r"\bgit\s+add\b[^\n;&|]*(\s-A\b|\s--all\b|\s\.(?=\s|$))") and "snapshot" not in command:
        deny("Stage named paths, never git add -A or git add . — only the atomic-commit snapshot is taken that way.")
    if has(r"\bgit\s+commit\b"):
        timeout = tool_input.get("timeout") or 120000
        if tool_input.get("run_in_background") or timeout < 600000:
            deny("Run git commit in the foreground with timeout 600000: pre-commit stashes unstaged work, and a commit killed mid-hook strands it (AGENTS.md, Never poll for a command I started).")
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
        missing = missing_signoffs(cwd)
        if missing:
            ask("Push before " + ", ".join(missing) + " signed off HEAD? A human reads what is pushed, so the branch is split into its commits first. The review agents run alongside them and do not hold this up.")
    if has(r"\bgh\s+pr\s+create\b"):
        problems = pull_request_problems(cwd, gh_body(command, cwd), True, hook_input)
        if problems:
            deny("Pull request: " + "; ".join(problems))
    if has(r"\bgh\s+pr\s+merge\b") or (has(r"\bgh\s+api\b") and has(r"/merge\b")):
        ask("Merging is the one gate that may not be skipped: did the user approve this merge, explicitly, for this pull request?")


if __name__ == "__main__":
    main()
