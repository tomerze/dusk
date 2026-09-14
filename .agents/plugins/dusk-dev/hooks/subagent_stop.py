import re

import report
from harness import (
    session_state,
    amend,
    holding,
    session_state,
    replace_text,
    agent_type,
    block,
    branch_name,
    current_branch,
    decisions_path,
    drive_state,
    field,
    findings,
    git,
    reviewed,
    tip,
    lint_branch,
    read_input,
    review_dir,
    save_json,
)


def raised(drive, values):
    if values["VERDICT"] == "ask-human":
        drive.setdefault("pending_questions", []).append(values["DECISION"])
    drive.pop("ranked_sha", None)


def stopped(session, name):
    waiting = session.get("agents") or []
    if name in waiting:
        waiting.remove(name)
    session["agents"] = waiting


def finished(hook_input):
    session_path, _ = session_state(hook_input.get("session_id", ""))
    amend(session_path, lambda session: stopped(session, agent_type(hook_input)))


def record(cwd, key, branch=None):
    branch = branch or current_branch(cwd)
    path, drive = drive_state(branch, cwd)
    drive[key] = tip(cwd, branch)
    save_json(path, drive)


def clear(cwd, key, branch=None):
    branch = branch or current_branch(cwd)
    path, drive = drive_state(branch, cwd)
    drive.pop(key, None)
    save_json(path, drive)


def atomic_commit(cwd, message, hook_input):
    code, snapshot = git(cwd, "rev-parse", "-q", "--verify", "harness-snapshot")
    if code != 0:
        block("Tag the final tree before splitting: git tag -f harness-snapshot HEAD. The tag is the proof the split lost nothing.")
    _, dirty = git(cwd, "status", "--porcelain")
    if dirty:
        block("The tree is not clean after the split:\n" + dirty)
    _, diff = git(cwd, "diff", "--stat", "harness-snapshot", "HEAD")
    if diff:
        block("The split changed the tree; git diff harness-snapshot HEAD must be empty:\n" + diff)
    problems = lint_branch(cwd)
    if problems:
        block("Commits that do not pass: " + "; ".join(problems))
    record(cwd, "split_sha", reviewed(cwd, hook_input))
    git(cwd, "tag", "-d", "harness-snapshot")


def self_review(cwd, message, hook_input):
    verdict = field(message, "VERDICT")
    if verdict not in ("pass", "findings"):
        block("End the review with `VERDICT: pass` or `VERDICT: findings`, after the findings in the three-line form.")
    if verdict == "pass":
        record(cwd, "reviewed_sha", reviewed(cwd, hook_input))
    else:
        clear(cwd, "reviewed_sha", reviewed(cwd, hook_input))


def race_screen(cwd, message, hook_input):
    race = field(message, "RACE")
    if race not in ("none", "possible"):
        block("End with `RACE: none` or `RACE: possible`; when possible, dispatch the race-inspector agent and relay its `RACES:` line.")
    races = field(message, "RACES")
    if race == "possible" and races not in ("none", "found"):
        block("A possible race needs the race-inspector agent's verdict: relay its `RACES: none` or `RACES: found` line.")
    if race == "none" or races == "none":
        record(cwd, "race_sha", reviewed(cwd, hook_input))
    else:
        clear(cwd, "race_sha", reviewed(cwd, hook_input))


def race_inspector(cwd, message, hook_input):
    if field(message, "RACES") not in ("none", "found"):
        block("End with `RACES: none` or `RACES: found`, each race written as interleaving, state, consequence, fix.")


def comment_review(cwd, message, hook_input):
    if field(message, "COMMENTS") not in ("none", "found"):
        block("End with `COMMENTS: none` or `COMMENTS: found`; each finding as `- <file>:<line> - <the comment> - <why it belongs in the commit message>`.")


def dilemma_triage(cwd, message, hook_input):
    names = ("DECISION", "DECIDED", "ALTERNATIVES", "REVERSAL", "VERDICT", "WHY")
    values = {name: field(message, name) for name in names}
    missing = [name for name, value in values.items() if not value]
    if missing:
        block("Answer in the six lines DECISION, DECIDED, ALTERNATIVES, REVERSAL, VERDICT, WHY; missing: " + ", ".join(missing))
    if values["VERDICT"] not in ("decide-alone", "ask-human"):
        block("VERDICT is `decide-alone` or `ask-human`.")
    branch = reviewed(cwd, hook_input)
    path = decisions_path(cwd, branch)
    path.parent.mkdir(parents=True, exist_ok=True)
    with holding(path):
        if not path.exists():
            path.write_text(f"# Decisions on {branch_name(branch)}\n")
        with path.open("a") as decisions:
            decisions.write(
                f"\n## {values['DECISION']}\n"
                f"- decided: {values['DECIDED']}\n"
                f"- alternatives: {values['ALTERNATIVES']}\n"
                f"- reversal: {values['REVERSAL']}\n"
                f"- triage: {values['VERDICT']} - {values['WHY']}\n"
            )
    drive_path, _ = drive_state(branch, cwd)
    amend(drive_path, lambda drive: raised(drive, values))
    report.render(cwd, branch)


def decision_ranker(cwd, message, hook_input):
    match = re.search(r"^RANKED:\s*\n((?:\s*\d+\..*\n?)+)", message or "", re.M)
    if not match:
        block("End with `RANKED:` followed by one numbered line per decision: `1. <title> - attention: high|medium|low - <why>`.")
    branch = reviewed(cwd, hook_input)
    path = decisions_path(cwd, branch)
    if not path.exists():
        block(f"{path} does not exist; nothing to rank.")
    with holding(path):
        text = re.sub(r"^## Ranked\n.*?(?=^## |\Z)", "", path.read_text(), flags=re.S | re.M)
        heading_end = text.find("\n") + 1
        ranked = "\n## Ranked\n\n" + match.group(1).strip() + "\n"
        replace_text(path, text[:heading_end] + ranked + text[heading_end:])
    report.render(cwd, branch)
    record(cwd, "ranked_sha", branch)


def write_review(cwd, name, title, message, branch):
    directory = review_dir(cwd, branch)
    directory.mkdir(parents=True, exist_ok=True)
    lines = findings(message)
    body = "\n".join(lines) if lines else "- none"
    (directory / name).write_text(f"# {title} {branch_name(branch)}\n\n{body}\n")
    report.render(cwd, branch)


def terminology_review(cwd, message, hook_input):
    terms = field(message, "TERMS")
    if terms not in ("none", "found") or (terms == "found" and not findings(message)):
        block("End with `TERMS: none` or `TERMS: found`; each term as `- <term> - <file>:<line> - <what it names> - <the word the codebase already has for it, or none>`.")
    branch = reviewed(cwd, hook_input)
    write_review(cwd, "terminology.md", "Terminology introduced on", message, branch)
    record(cwd, "terminology_sha", branch)


def string_review(cwd, message, hook_input):
    strings = field(message, "STRINGS")
    if strings not in ("none", "found") or (strings == "found" and not findings(message)):
        block("End with `STRINGS: none` or `STRINGS: found`; each string as `- <file>:<line> - \"<string>\" - <where a person sees it>`.")
    branch = reviewed(cwd, hook_input)
    write_review(cwd, "strings.md", "User-facing strings introduced on", message, branch)
    record(cwd, "strings_sha", branch)


HANDLERS = {
    "atomic-commit": atomic_commit,
    "self-review": self_review,
    "race-screen": race_screen,
    "race-inspector": race_inspector,
    "comment-review": comment_review,
    "dilemma-triage": dilemma_triage,
    "decision-ranker": decision_ranker,
    "terminology-review": terminology_review,
    "string-review": string_review,
}


def main():
    hook_input = read_input()
    finished(hook_input)
    handler = HANDLERS.get(agent_type(hook_input))
    if handler:
        handler(hook_input.get("cwd", "."), hook_input.get("last_assistant_message") or "", hook_input)


if __name__ == "__main__":
    main()
