from harness import context, current_branch, decisions_path, drive_state, read_input

PREAMBLE = """dusk-dev: the dusk harness is active.
- Work in this repository goes through /dusk-dev:activate, which puts the rules below in force on the branch that is checked out. Issue work goes through /dusk-dev:drive-issue, which is activate plus the issue. /what reports where things stand; /honest-to-god before reporting a finding, a risk or a limitation.
- Hooks enforce, as the action happens: nothing personal written anywhere (no name, no email address, no home directory, no account handle); no comment lines written by me; commit subjects with no feat:-style prefix, no Say, no (#N), no watermark trailer; git commit in the foreground with timeout 600000; no polling loops; no cp of a target directory; no bare git stash; cargo test only after asking.
- A commit whose diff adds a comment line is read by the comment-review agent; a comment it finds that belongs in the commit message comes back to be moved there. A commit that adds none is recorded as not reviewed, without the agent.
- Work goes through three phases. Writing: the rules above, and the dilemma-triage agent on every decision. Getting ready to review: the atomic-commit agent splits the branch, which is what gates the hand-off, because a human reads commits and not a blob; pushing itself is never held up. Ready for review: the hand-off goes out and the review agents run alongside the human, never ahead of them.
- The review agents are self-review, race-screen, terminology-review, string-review and decision-ranker. They gate nothing; what they find goes on the pull request as it lands, and the branch's review folder collects it: decisions.md, comments.md, terminology.md, strings.md and report.html, whose locations open in VS Code.
- Once this session has committed or pushed, every turn is marked, one of three ways. A hand-off ends with `Ready for review.` on its own line and nothing after it. A dilemma opens with a `DILEMMA:` line and asks the one question that blocks the work; the dilemma-screen agent reads it first and sends back anything I could have decided and recorded, so do not stop for a name, a string, a commit split or permission I already have. A wait opens with `WAITING ON SUBAGENTS:` and names them, and is refused unless one this session dispatched is still running. An answer to something the user asked ends with `Still ready for review.` or `Still waiting for dilemma verdict.`, whichever is standing. Nothing else ends a turn; the status line shows which state you are in. `Ready for review.` is verified: PR open at the pushed tip, worktree gone. Merging always asks."""


def main():
    hook_input = read_input()
    cwd = hook_input.get("cwd", ".")
    branch = current_branch(cwd)
    lines = [PREAMBLE]
    if branch and branch not in ("master", "main"):
        _, drive = drive_state(branch, cwd)
        if drive:
            parts = [f"Drive in flight on `{branch}`: phase {drive.get('phase', 'working')}"]
            if drive.get("issue"):
                parts.append(f"issue #{drive['issue']}")
            if drive.get("pr"):
                parts.append(f"PR #{drive['pr']}")
            lines.append("; ".join(parts) + ".")
        path = decisions_path(cwd, branch)
        if path.exists():
            lines.append(f"Decisions so far: {path.relative_to(cwd)}")
    context("\n".join(lines))


if __name__ == "__main__":
    main()
