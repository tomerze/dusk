import re

from harness import (
    agent_type,
    ask,
    deny,
    lint_branch,
    lint_message,
    missing_signoffs,
    read_input,
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
        deny("Never poll for a command you started: the harness re-invokes you when a background command exits (CLAUDE.md, Never poll for a command I started).")
    if has(r"\bcp\b[^\n;&|]*\btarget\b"):
        deny("Never copy a target directory; park it with mv and move it back (drive-issue, Step 2).")
    if has(r"\bgit\s+stash\b(?!\s+(push|list|show|apply|drop|branch)\b)"):
        deny("No bare git stash and no stash pop: the stash stack is shared with every worktree. Use git stash push -u -m <tag>, restore with git stash apply <sha>, then drop it by tag.")
    if has(r"\bcargo\s+(test|nextest)\b"):
        ask("Tests run only when the user asked for them (CLAUDE.md, Don't write or run tests unless told). Did they ask?")
    if has(r"\bgit\s+add\b[^\n;&|]*(\s-A\b|\s--all\b|\s\.(?=\s|$))") and "snapshot" not in command:
        deny("Stage named paths, never git add -A or git add . — only the atomic-commit snapshot is taken that way.")
    if has(r"\bgit\s+commit\b"):
        timeout = tool_input.get("timeout") or 120000
        if tool_input.get("run_in_background") or timeout < 600000:
            deny("Run git commit in the foreground with timeout 600000: pre-commit stashes unstaged work, and a commit killed mid-hook strands it (CLAUDE.md, Never poll for a command I started).")
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


if __name__ == "__main__":
    main()
