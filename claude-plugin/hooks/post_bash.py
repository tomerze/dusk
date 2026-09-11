import json
import re

import report
from harness import (
    branch_name,
    current_branch,
    drive_state,
    field,
    findings,
    git,
    head,
    headless_agent,
    read_input,
    review_dir,
    save_json,
    session_state,
)

COMMENT_PROMPT = (
    "Review commit {sha} on the current branch: run `git show {sha}` and judge every "
    "comment line it adds or changes."
)


def only_review_files(cwd):
    _, names = git(cwd, "show", "--name-only", "--format=", "HEAD")
    paths = [name for name in names.splitlines() if name.strip()]
    return bool(paths) and all(name.startswith("review/") for name in paths)


def review_commit(cwd, branch, sha, subject):
    answer = headless_agent("comment-review", COMMENT_PROMPT.format(sha=sha), cwd)
    if answer is None or field(answer, "COMMENTS") not in ("none", "found"):
        return
    found = findings(answer)
    directory = review_dir(cwd, branch)
    directory.mkdir(parents=True, exist_ok=True)
    comments = directory / "comments.md"
    if not comments.exists():
        comments.write_text(f"# Comments that belong in commit messages on {branch_name(branch)}\n")
    with comments.open("a") as file:
        file.write(f"\n## {sha[:8]} {subject}\n" + ("\n".join(found) if found else "- none") + "\n")
    report.render(cwd, branch)
    if found:
        print(
            json.dumps(
                {
                    "hookSpecificOutput": {
                        "hookEventName": "PostToolUse",
                        "additionalContext": (
                            f"comment-review: these comment lines in {sha[:8]} belong in the commit "
                            "message, not in the code. Move them into the message and amend:\n"
                            + "\n".join(found)
                        ),
                    }
                }
            )
        )


def main():
    hook_input = read_input()
    command = hook_input.get("tool_input", {}).get("command", "")
    cwd = hook_input.get("cwd", ".")
    branch = current_branch(cwd)
    if not branch or branch in ("master", "main", "HEAD"):
        return
    _, session = session_state(hook_input.get("session_id", ""))
    path, drive = drive_state(branch)
    changed = False
    if re.search(r"\bgit\s+commit\b", command):
        if session.get("drive") and not drive.get("issue"):
            drive["issue"] = session["drive"].get("issue")
            drive["phase"] = drive.get("phase") or "working"
            changed = True
        sha = head(cwd)
        _, subject = git(cwd, "log", "-1", "--format=%s")
        if sha and not re.match(r"^WIP\b", subject, re.I) and not only_review_files(cwd):
            review_commit(cwd, branch, sha, subject)
    if re.search(r"\bgit\s+push\b", command):
        drive["phase"] = "pushed"
        changed = True
    if changed:
        save_json(path, drive)


if __name__ == "__main__":
    main()
