import os
import re
from pathlib import Path

from harness import (
    DILEMMA_LINE,
    READY_LINE,
    REPORT_HEADINGS,
    STANDING_LINE,
    WAITING_LINE,
    READY_SIGNOFFS,
    REVIEW_SIGNOFFS,
    block,
    branch_name,
    current_branch,
    decisions_path,
    drive_state,
    field,
    git,
    headless_agent,
    tip,
    missing_signoffs,
    pull_request_view,
    read_input,
    save_json,
    session_state,
)

MAX_BLOCKS = 3
DILEMMA_EXCERPT = 4000
SCREENED = "screened"
SCREEN_PROMPT = (
    "The main agent is about to end its turn with the message below, which stops the "
    "session until a person answers it. Judge whether that question genuinely blocks "
    "the work.\n\n<message>\n{message}\n</message>"
)


def excerpt(message):
    if len(message) <= DILEMMA_EXCERPT:
        return message
    half = DILEMMA_EXCERPT // 2
    return message[:half] + "\n\n[…cut…]\n\n" + message[-half:]


def stalling(cwd, message):
    if os.environ.get("DUSK_NO_DILEMMA_SCREEN"):
        return None
    answer = headless_agent("dilemma-screen", SCREEN_PROMPT.format(message=excerpt(message)), cwd)
    if answer is None or field(answer, "VERDICT") != "not-a-blocker":
        return None
    why = field(answer, "WHY") or "it is the kind of call the agent makes and records"
    return why if why.endswith((".", "!", "?")) else why + "."


def problem(text, key=None):
    return (key or text, text)


def answered(path, title):
    if not path.exists():
        return False
    section = re.search(rf"^## {re.escape(title)}\n(.*?)(?=^## |\Z)", path.read_text(), re.S | re.M)
    return bool(section and re.search(r"^- answer:", section.group(1), re.M))


READY_ALONE = re.compile(rf"^[ \t]*{re.escape(READY_LINE)}[ \t]*$", re.M)
DILEMMA_MARK = re.compile(rf"^[ \t]*\**{re.escape(DILEMMA_LINE)}", re.M)
WAITING_MARK = re.compile(rf"^[ \t]*\**{re.escape(WAITING_LINE)}", re.M)


def waits_on_agents(message):
    lines = [line for line in message.strip().splitlines() if line.strip()]
    return bool(lines) and bool(WAITING_MARK.match(lines[0]))


def reports_state(message):
    lines = [line for line in message.strip().splitlines() if line.strip()]
    return bool(lines) and lines[0].strip() == REPORT_HEADINGS[0] and all(
        heading in message for heading in REPORT_HEADINGS[1:]
    )


def restates_standing(message):
    lines = [line for line in message.strip().splitlines() if line.strip()]
    last = lines[-1].strip() if lines else ""
    for state, line in STANDING_LINE.items():
        if last == line:
            return state
    return None


def marks_dilemma(message):
    lines = [line for line in message.strip().splitlines() if line.strip()]
    return bool(lines) and bool(DILEMMA_MARK.match(lines[0])) and "?" in message


def claims_ready(message):
    lines = [line for line in message.strip().splitlines() if line.strip()]
    last = bool(lines) and lines[-1].strip() == READY_LINE
    return last or bool(READY_ALONE.search(message)), last


def ready_problems(cwd, branch, last):
    problems = []
    url = None
    if not last:
        problems.append(f"`{READY_LINE}` is the last line of the message and nothing follows it")
    if "/.claude/worktrees/" in str(Path(cwd).resolve()):
        problems.append("the worktree is still in place; the branch is not free for the user to check out")
    remote = branch_name(branch)
    _, ahead = git(cwd, "rev-list", "--count", f"origin/{remote}..{branch}")
    if ahead and ahead != "0":
        problems.append(f"{ahead} commit(s) not pushed")
    pull_request = pull_request_view(cwd, remote)
    if not pull_request:
        problems.append(f"no pull request found for {remote}")
    else:
        url = pull_request.get("url")
        if pull_request.get("state") != "OPEN":
            problems.append(f"PR #{pull_request['number']} is {pull_request.get('state')}, not open")
        if pull_request.get("headRefOid") != tip(cwd, branch):
            problems.append(f"PR #{pull_request['number']} does not carry the local tip")
        if "## Decisions" not in (pull_request.get("body") or ""):
            problems.append("the PR body does not carry the ranked decisions under `## Decisions`")
    unsplit = missing_signoffs(cwd, READY_SIGNOFFS, branch)
    if unsplit:
        problems.append(
            ", ".join(unsplit) + " has not signed off this tip. A person cannot read a branch that "
            "is one blob, so it is cut into the commits the work is before they are asked to."
        )
    return problems, url


def main():
    hook_input = read_input()
    cwd = hook_input.get("cwd", ".")
    message = hook_input.get("last_assistant_message") or ""
    branch = current_branch(cwd)
    if not branch:
        return
    session_path, session = session_state(hook_input.get("session_id", ""))
    branch = session.get("handed") or branch
    drive_path, drive = drive_state(branch, cwd)
    problems = []
    allowed_standing = None

    pending = [title for title in drive.get("pending_questions", []) if not answered(decisions_path(cwd, branch), title)]
    if pending and not marks_dilemma(message):
        problems.append(problem("dilemma-triage said these need the user: " + "; ".join(pending) + f". Ask it under a `{DILEMMA_LINE}` line, with a question mark, and record the answer as `- answer:` under the decision."))

    claimed, last = claims_ready(message)
    asked = session["asked"] if "asked" in session else drive.get("pending_questions", [])
    if session.get("standing") == "dilemma" and asked and all(answered(decisions_path(cwd, branch), title) for title in asked):
        session["standing"] = "ready" if session.get("handed_off") == branch else None
    standing = session.get("standing")
    if claimed and session.get("handed_off") == branch:
        problems.append(problem(
            f"`{READY_LINE}` goes out once per branch, and it already has. The human is reading "
            f"the pull request now. A fix after that is committed, pushed, and said on the pull "
            f"request; the turn ends `{STANDING_LINE['ready']}`. Handing the same branch over "
            "again restarts a review that never stopped."
        ))
    elif claimed:
        ready, pull_request_url = ready_problems(cwd, branch, last)
        problems.extend(problem(text) for text in ready)
        if not problems:
            waiting = missing_signoffs(cwd, REVIEW_SIGNOFFS, branch)
            if waiting:
                problems.append(problem(
                    "the hand-off stands and the human is reading it. Now run the agents that "
                    "review alongside them - " + ", ".join(waiting) + " - and put what they find "
                    "on the pull request as it lands."
                ))
        if not problems:
            session["standing"] = "ready"
            session["handed_off"] = branch
            session["pull_request"] = pull_request_url
    elif marks_dilemma(message):
        why = None if pending else stalling(cwd, message)
        if why:
            problems.append(problem(
                f"the question does not block the work: {why} Decide it, record it in the reply, "
                "and carry on - the user reads it in review, in one pass, instead of being "
                "interrupted for it now.",
                SCREENED,
            ))
            allowed_standing = "dilemma"
        else:
            session["standing"] = "dilemma"
            session["asked"] = pending
    elif reports_state(message):
        pass
    elif waits_on_agents(message):
        if not session.get("agents"):
            problems.append(problem(
                f"`{WAITING_LINE}` says an agent this session dispatched is still running. None is. "
                "Carry on with the work, or end the turn the way its state actually is."
            ))
    elif session.get("committed") or drive.get("phase") in ("pushed", "opened"):
        restated = restates_standing(message)
        if restated is None:
            problems.append(problem(
                f"This session has changed the history, so the turn is marked. It ends with "
                f"`{READY_LINE}` on its own line and nothing after it, or it opens with a "
                f"`{DILEMMA_LINE}` line and asks the one question that blocks the work, or with "
                f"`{WAITING_LINE}` naming the agents it is waiting on, or it answers what the user "
                f"asked and ends with the line that says nothing else moved: "
                f"`{STANDING_LINE['ready']}` or `{STANDING_LINE['dilemma']}`."
            ))
        elif standing is None:
            problems.append(problem(
                f"`{STANDING_LINE[restated]}` says an earlier turn handed something over or asked "
                f"something. None did. End with `{READY_LINE}` or a `{DILEMMA_LINE}` question."
            ))
        elif restated != standing:
            problems.append(problem(
                f"what stands is the {standing}, so the line is `{STANDING_LINE[standing]}`, not "
                f"`{STANDING_LINE[restated]}`."
            ))

    if not problems:
        session["stop_blocks"] = {}
        save_json(session_path, session)
        return
    key = "|".join(name for name, _ in problems)
    count = session["stop_blocks"].get(key, 0) + 1
    session["stop_blocks"] = {key: count}
    if count > MAX_BLOCKS:
        if allowed_standing:
            session["standing"] = allowed_standing
            session["asked"] = []
        save_json(session_path, session)
        return
    save_json(session_path, session)
    block("Not yet: " + " ".join(text for _, text in problems))


if __name__ == "__main__":
    main()
