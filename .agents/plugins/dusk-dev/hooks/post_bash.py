import json
import re

import report
from harness import (
    holding,
    branch_name,
    current_branch,
    drive_state,
    field,
    findings,
    git,
    tip,
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


HASH_SUFFIXES = (".py", ".capnp", ".toml", ".yaml", ".yml", ".sh", ".cfg", ".ini", ".conf")
SLASH_SUFFIXES = (".rs", ".c", ".h", ".cc", ".cpp", ".hpp", ".js", ".ts", ".go", ".java")
MARKUP_SUFFIXES = (".md", ".markdown", ".html", ".xml")
HASH_COMMENT = re.compile(r"^\s*#|\S\s+#")
SLASH_COMMENT = re.compile(r"^\s*//|/\*|\S\s+//")
MARKUP_COMMENT = re.compile(r"<!--|^\s*//|/\*|\S\s+//")
ANY_COMMENT = re.compile(r"^\s*(//|#)|/\*|<!--|\S\s+(//|#)")


def comment_pattern(path):
    if path.endswith(HASH_SUFFIXES):
        return HASH_COMMENT
    if path.endswith(SLASH_SUFFIXES):
        return SLASH_COMMENT
    if path.endswith(MARKUP_SUFFIXES):
        return MARKUP_COMMENT
    return ANY_COMMENT


def needs_review(cwd, sha):
    code, lineage = git(cwd, "rev-list", "-1", "--parents", sha)
    if code != 0 or len(lineage.split()) > 2:
        return True
    code, diff = git(cwd, "show", "-U0", "--format=", sha)
    if code != 0:
        return True
    pattern = ANY_COMMENT
    for line in diff.splitlines():
        if line.startswith("+++ "):
            pattern = comment_pattern(line[4:])
        elif line.startswith("+") and pattern.search(line[1:]):
            return True
    return False


def pushed_branch(command):
    match = re.search(r"\bgit\s+push\b([^\n;&|]*)", command)
    if not match:
        return None
    words = [word for word in match.group(1).split() if not word.startswith("-")]
    if len(words) < 2:
        return None
    return words[1].split(":")[-1]


def review_commit(cwd, branch, sha, subject):
    reviewed = needs_review(cwd, sha)
    if reviewed:
        answer = headless_agent("comment-review", COMMENT_PROMPT.format(sha=sha), cwd)
        if answer is None or field(answer, "COMMENTS") not in ("none", "found"):
            return
        found = findings(answer)
    else:
        found = []
    directory = review_dir(cwd, branch)
    directory.mkdir(parents=True, exist_ok=True)
    comments = directory / "comments.md"
    if not comments.exists():
        comments.write_text(f"# Comments that belong in commit messages on {branch_name(branch)}\n")
    if found:
        entry = "\n".join(found)
    elif reviewed:
        entry = "- none"
    else:
        entry = "- not reviewed: no added line matched a comment pattern"
    with holding(comments), comments.open("a") as file:
        file.write(f"\n## {sha[:8]} {subject}\n" + entry + "\n")
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
    session_path, session = session_state(hook_input.get("session_id", ""))
    path, drive = drive_state(branch, cwd)
    changed = False
    if re.search(r"\bgit\s+(commit|push)\b", command) and not session.get("committed"):
        _, last = git(cwd, "reflog", "-1", "--format=%gs")
        if last.startswith("commit") or re.search(r"\bgit\s+push\b", command):
            session["committed"] = True
            save_json(session_path, session)
    if re.search(r"\bgit\s+commit\b", command):
        if session.get("drive") and not drive.get("issue"):
            drive["issue"] = session["drive"].get("issue")
            drive["phase"] = drive.get("phase") or "working"
            changed = True
        sha = tip(cwd)
        _, subject = git(cwd, "log", "-1", "--format=%s")
        if sha and not re.match(r"^WIP\b", subject, re.I):
            review_commit(cwd, branch, sha, subject)
    if re.search(r"\bgit\s+push\b", command):
        drive["phase"] = "pushed"
        changed = True
        pushed = pushed_branch(command) or branch
        if session.get("handed") != pushed:
            session["handed"] = pushed
            save_json(session_path, session)
    if changed:
        save_json(path, drive)


if __name__ == "__main__":
    main()
