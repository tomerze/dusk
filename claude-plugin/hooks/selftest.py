import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

HOOKS = Path(__file__).resolve().parent


def run(script, hook_input):
    env = dict(os.environ, CLAUDE_PLUGIN_DATA=str(DATA), PYTHONPATH=str(HOOKS))
    completed = subprocess.run(
        [sys.executable, str(HOOKS / script)],
        input=json.dumps(hook_input),
        capture_output=True,
        text=True,
        env=env,
        check=False,
    )
    return completed.returncode, completed.stdout, completed.stderr


def bash(command, **tool_input):
    return {"session_id": "selftest", "cwd": str(REPO), "tool_name": "Bash", "tool_input": {"command": command, **tool_input}}


def edit(path, old, new):
    return {"session_id": "selftest", "cwd": str(REPO), "tool_name": "Edit", "tool_input": {"file_path": path, "old_string": old, "new_string": new}}


def agent(name, message):
    return {"session_id": "selftest", "cwd": str(REPO), "agent_type": f"dusk-dev:{name}", "last_assistant_message": message}


def expect(name, result, code, contains=None):
    actual, stdout, stderr = result
    text = stdout + stderr
    ok = actual == code and (contains is None or contains in text)
    print(("ok   " if ok else "FAIL ") + name + ("" if ok else f" -> exit {actual}: {text.strip()[:200]}"))
    return ok


def review_file(name, text, present=True):
    path = REPO / "review" / "topic" / name
    found = path.exists() and text in path.read_text()
    return (0 if found == present else 1, "", "")


with tempfile.TemporaryDirectory() as temp:
    DATA = Path(temp) / "data"
    REPO = Path(temp) / "repo"
    REPO.mkdir()
    subprocess.run(["git", "init", "-q", "-b", "master"], cwd=REPO, check=True)
    subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "Root"], cwd=REPO, check=True)
    subprocess.run(["git", "checkout", "-q", "-b", "topic"], cwd=REPO, check=True)
    triage = "DECISION: Keep x\nDECIDED: keep it\nALTERNATIVES: drop it\nREVERSAL: one line\nVERDICT: decide-alone\nWHY: local"
    results = [
        expect("commit with a prefix is denied", run("pre_bash.py", bash('git commit -m "feat: add x"', timeout=600000)), 2, "conventional-commit prefix"),
        expect("commit starting with Say is denied", run("pre_bash.py", bash('git commit -m "Say how it opens"', timeout=600000)), 2, "starts with Say"),
        expect("commit with a watermark is denied", run("pre_bash.py", bash('git commit -m "Add x" -m "Co-Authored-By: someone"', timeout=600000)), 2, "watermark"),
        expect("commit with a short timeout is denied", run("pre_bash.py", bash('git commit -m "Add x"')), 2, "timeout 600000"),
        expect("plain commit is allowed", run("pre_bash.py", bash('git commit -m "Add x"', timeout=600000)), 0),
        expect("git add -A is denied", run("pre_bash.py", bash("git add -A && git commit -m 'Add x'", timeout=600000)), 2, "named paths"),
        expect("the snapshot may use git add -A", run("pre_bash.py", bash("git add -A && git commit --no-verify -m 'WIP snapshot'", timeout=600000)), 0),
        expect("polling is denied", run("pre_bash.py", bash("until grep -q done out.log; do sleep 5; done")), 2, "Never poll"),
        expect("copying target is denied", run("pre_bash.py", bash("cp -a ../dusk/target .")), 2, "target"),
        expect("a heredoc written to a file is text", run("pre_bash.py", bash("cat > notes.md <<'EOF'\ncp -a ../dusk/target .\nEOF")), 0),
        expect("a heredoc fed to an interpreter is a command", run("pre_bash.py", bash("bash <<'EOF'\ncp -a ../dusk/target .\nEOF")), 2, "target directory"),
        expect("a heredoc piped into an interpreter is a command", run("pre_bash.py", bash("cat <<'EOF' | bash\ncp -a ../dusk/target .\nEOF")), 2, "target directory"),
        expect("the word snapshot in a heredoc does not exempt git add -A", run("pre_bash.py", bash("git add -A && cat > notes.md <<'EOF'\nsnapshot\nEOF", timeout=600000)), 2, "named paths"),
        expect("a path that merely contains sh does not make a heredoc a command", run("pre_bash.py", bash("cat > base/sh/README.md <<'EOF'\ngit stash\nEOF")), 0),
        expect("bare stash is denied", run("pre_bash.py", bash("git stash")), 2, "stash"),
        expect("stash push is allowed", run("pre_bash.py", bash("git stash push -u -m tag")), 0),
        expect("cargo test asks", run("pre_bash.py", bash("cargo nextest run")), 0, '"ask"'),
        expect("push to master is denied", run("pre_bash.py", bash("git push origin master")), 2, "master"),
        expect("push without sign-offs asks", run("pre_bash.py", bash("git push -u origin topic")), 0, '"ask"'),
        expect("a read-only agent may not commit", run("pre_bash.py", {**bash('git commit -m "Add x"', timeout=600000), "agent_type": "dusk-dev:self-review"}), 2, "reads"),
        expect("a comment line in Rust is denied", run("pre_edit.py", edit(str(REPO / "src/lib.rs"), "let a = 1;", "// the answer\nlet a = 1;")), 2, "do not write comments"),
        expect("an existing comment line passes", run("pre_edit.py", edit(str(REPO / "src/lib.rs"), "// kept\nlet a = 1;", "// kept\nlet a = 2;")), 0),
        expect("a program edit needs the authoring skill", run("pre_edit.py", edit(str(REPO / "base/sleep/src/lib.rs"), "a", "b")), 2, "authoring-a-program"),
        expect("a hash inside a python string is not a comment", run("pre_edit.py", edit(str(REPO / "hooks/x.py"), "a = 1", 'PROMPT = \"\"\"\n# Heading\n\"\"\"')), 0),
        expect("a markdown edit is free", run("pre_edit.py", edit(str(REPO / "docs/x.md"), "a", "# b")), 0),
        expect("a stop with nothing pending passes", run("stop.py", {"session_id": "selftest", "cwd": str(REPO), "last_assistant_message": "Done."}), 0),
        expect("a Ready line that is not last is blocked", run("stop.py", {"session_id": "selftest", "cwd": str(REPO), "last_assistant_message": "Ready for review.\n\nAlso this."}), 2, "last line"),
        expect("a quoted Ready line is not a claim", run("stop.py", {"session_id": "selftest", "cwd": str(REPO), "last_assistant_message": '- a.py:1 — "Ready for review." — the hand-off prints it'}), 0),
        expect("a malformed triage answer is blocked", run("subagent_stop.py", agent("dilemma-triage", "DECISION: x\nVERDICT: maybe")), 2, "six lines"),
        expect("a triage answer is recorded", run("subagent_stop.py", agent("dilemma-triage", triage)), 0),
        expect("the decision landed in the file", review_file("decisions.md", "## Keep x"), 0),
        expect("a ranking is recorded", run("subagent_stop.py", agent("decision-ranker", "RANKED:\n1. Keep x — attention: low — local")), 0),
        expect("the ranking heads the file", review_file("decisions.md", "## Ranked"), 0),
        expect("a review without a verdict is blocked", run("subagent_stop.py", agent("self-review", "looks fine")), 2, "VERDICT"),
        expect("a verdict the agent bolded is read", run("subagent_stop.py", agent("self-review", "**VERDICT: pass**")), 0),
        expect("the split needs the snapshot tag", run("subagent_stop.py", agent("atomic-commit", "done")), 2, "harness-snapshot"),
        expect("a comment review without a verdict is blocked", run("subagent_stop.py", agent("comment-review", "nothing")), 2, "COMMENTS"),
        expect("a terminology review is recorded", run("subagent_stop.py", agent("terminology-review", "- shard — src/a.rs:12 — a partition — none\n**TERMS: found**")), 0),
        expect("the term landed in the file", review_file("terminology.md", "- shard — src/a.rs:12"), 0),
        expect("a string review is recorded", run("subagent_stop.py", agent("string-review", '- I looked at every crate\n- src/b.rs:7 — "no such pid" — kill prints it\nSTRINGS: found')), 0),
        expect("the narration was dropped", review_file("strings.md", "- I looked", present=False), 0),
        expect("the string landed in the file", review_file("strings.md", '"no such pid"'), 0),
        expect("the report links a location to vscode", review_file("report.html", 'data-file="src/a.rs" data-line="12"'), 0),
        expect("the report carries every section", review_file("report.html", "User-facing strings introduced"), 0),
    ]
    sys.exit(0 if all(results) else 1)
