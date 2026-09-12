import re
from pathlib import Path

from harness import (
    DILEMMA_LINE,
    READY_LINE,
    REVIEW_SIGNOFFS,
    block,
    branch_name,
    current_branch,
    decisions_path,
    drive_state,
    git,
    head,
    missing_signoffs,
    pull_request_view,
    read_input,
    save_json,
    session_state,
)

MAX_BLOCKS = 3


def answered(path, title):
    if not path.exists():
        return False
    section = re.search(rf"^## {re.escape(title)}\n(.*?)(?=^## |\Z)", path.read_text(), re.S | re.M)
    return bool(section and re.search(r"^- answer:", section.group(1), re.M))


READY_ALONE = re.compile(rf"^[ \t]*{re.escape(READY_LINE)}[ \t]*$", re.M)
DILEMMA_MARK = re.compile(rf"^[ \t]*\**{re.escape(DILEMMA_LINE)}", re.M)


def marks_dilemma(message):
    lines = [line for line in message.strip().splitlines() if line.strip()]
    return bool(lines) and bool(DILEMMA_MARK.match(lines[0])) and "?" in message


def claims_ready(message):
    lines = [line for line in message.strip().splitlines() if line.strip()]
    last = bool(lines) and lines[-1].strip() == READY_LINE
    return last or bool(READY_ALONE.search(message)), last


def ready_problems(cwd, branch, last):
    problems = []
    if not last:
        problems.append(f"`{READY_LINE}` is the last line of the message and nothing follows it")
    if "/.claude/worktrees/" in str(Path(cwd).resolve()):
        problems.append("the worktree is still in place; the branch is not free for the user to check out")
    remote = branch_name(branch)
    _, ahead = git(cwd, "rev-list", "--count", f"origin/{remote}..HEAD")
    if ahead and ahead != "0":
        problems.append(f"{ahead} commit(s) not pushed")
    pull_request = pull_request_view(cwd, remote)
    if not pull_request:
        problems.append(f"no pull request found for {remote}")
    else:
        if pull_request.get("state") != "OPEN":
            problems.append(f"PR #{pull_request['number']} is {pull_request.get('state')}, not open")
        if pull_request.get("headRefOid") != head(cwd):
            problems.append(f"PR #{pull_request['number']} does not carry the local tip")
        if "## Decisions" not in (pull_request.get("body") or ""):
            problems.append("the PR body does not carry the ranked decisions under `## Decisions`")
    return problems


def main():
    hook_input = read_input()
    cwd = hook_input.get("cwd", ".")
    message = hook_input.get("last_assistant_message") or ""
    branch = current_branch(cwd)
    if not branch:
        return
    session_path, session = session_state(hook_input.get("session_id", ""))
    drive_path, drive = drive_state(branch)
    problems = []

    pending = [title for title in drive.get("pending_questions", []) if not answered(decisions_path(cwd, branch), title)]
    if pending and not marks_dilemma(message):
        problems.append("dilemma-triage said these need the user: " + "; ".join(pending) + f". Ask it under a `{DILEMMA_LINE}` line, with a question mark, and record the answer as `- answer:` under the decision.")

    claimed, last = claims_ready(message)
    if claimed:
        problems.extend(ready_problems(cwd, branch, last))
        if not problems:
            pending = missing_signoffs(cwd, REVIEW_SIGNOFFS)
            if pending:
                problems.append(
                    "the hand-off stands and the human is reading it. Now run the agents that "
                    "review alongside them — " + ", ".join(pending) + " — and put what they find "
                    "on the pull request as it lands."
                )
    elif (session.get("committed") or drive.get("phase") in ("pushed", "opened")) and not marks_dilemma(message):
        problems.append(
            f"This session has changed the history, so the turn is marked. It ends with "
            f"`{READY_LINE}` on its own line and nothing after it, or it opens with a "
            f"`{DILEMMA_LINE}` line and asks the one question that blocks the work. A report that "
            "is neither leaves the user with nothing to do."
        )

    if not problems:
        session["stop_blocks"] = {}
        save_json(session_path, session)
        return
    key = "|".join(problems)
    count = session["stop_blocks"].get(key, 0) + 1
    session["stop_blocks"] = {key: count}
    save_json(session_path, session)
    if count > MAX_BLOCKS:
        return
    block("Not yet: " + " ".join(problems))


if __name__ == "__main__":
    main()
