PREAMBLE = """dusk-dev: the dusk harness is active.
- Issue work goes through /dusk-dev:drive-issue. /what reports where things stand; /honest-to-god before reporting a finding, a risk or a limitation.
- Hooks enforce, as the action happens: no comment lines written by me; commit subjects with no feat:-style prefix, no Say, no (#N), no watermark trailer; git commit in the foreground with timeout 600000; no polling loops; no cp of a target directory; no bare git stash; cargo test only after asking.
- A push waits for the atomic-commit agent to have split the tip: a human reads what is pushed, and nobody can read a WIP blob."""


from harness import context, current_branch, drive_state, read_input

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
    context("\n".join(lines))


if __name__ == "__main__":
    main()
