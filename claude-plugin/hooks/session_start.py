PREAMBLE = """dusk-dev: the dusk harness is active.
- Issue work goes through /dusk-dev:drive-issue. /what reports where things stand; /honest-to-god before reporting a finding, a risk or a limitation.
- Hooks enforce, as the action happens: no comment lines written by me; commit subjects with no feat:-style prefix, no Say, no (#N), no watermark trailer; git commit in the foreground with timeout 600000; no polling loops; no cp of a target directory; no bare git stash; cargo test only after asking.
- Every commit is read by the comment-review agent; a comment it finds that belongs in the commit message comes back to be moved there.
- A push and a pull request wait for three sign-offs on HEAD: the atomic-commit agent's split, the self-review agent, the race-screen agent. Merging always asks.
- Every decision goes through the dilemma-triage agent as it is made; it lands in review/<branch>/decisions.md. Before the PR, the decision-ranker agent ranks them, and the PR body carries the ranking.
- A hand-off ends with `Ready for review.` on its own line and nothing after it, or asks the blocking question. The line is verified: PR open at the pushed tip, worktree gone, decisions on the branch."""


from harness import context, current_branch, decisions_path, drive_state, read_input

def main():
    hook_input = read_input()
    cwd = hook_input.get("cwd", ".")
    branch = current_branch(cwd)
    lines = [PREAMBLE]
    if branch and branch not in ("master", "main"):
        _, drive = drive_state(branch)
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
